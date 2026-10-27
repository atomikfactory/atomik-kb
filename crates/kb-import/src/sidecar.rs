//! The `atomik-export.json` sidecar of an atomik-kb Markdown archive (ADR-0036, sections 2.7 and
//! 3.7): the property schema (kinds, options and colours, relations, rollups, formulas) and the pages
//! in tree order.

use kb_types::properties::TAGS_NAME;
use kb_types::{OptionColor, PropertyKind, RollupFunction, StatusGroup};
use serde_json::Value;

use crate::frontmatter::MAX_NAME_CHARS;
use crate::paths::truncate_chars;
use crate::{PlannedConfig, PlannedOption, PlannedProperty, PlannedRollupTarget};

/// Most options read for one property (the schema's limit).
const MAX_OPTIONS: usize = 500;

/// What the sidecar holds.
#[derive(Debug, Default)]
pub(crate) struct Sidecar {
    /// The schema, in order.
    pub properties: Vec<PlannedProperty>,
    /// The names of the properties the sidecar declared as `person` (ADR-0049), which are planned
    /// as text: an export names its people but never gives their account ids, so there is nobody
    /// to import (`docs/architecture/import-export.md`, "Annotations and atomik-kb archives").
    pub people: Vec<String>,
    /// The page files in tree order.
    pub pages: Vec<String>,
}

/// Reads a sidecar. `None` when the text is not JSON or not an object; entries that cannot be
/// read are left out.
pub(crate) fn parse(text: &str) -> Option<Sidecar> {
    let root: Value = serde_json::from_str(text).ok()?;
    let root = root.as_object()?;
    let mut sidecar = Sidecar::default();
    if let Some(Value::Array(list)) = root.get("properties") {
        for entry in list {
            if let Some((property, person)) = property(entry)
                && !sidecar
                    .properties
                    .iter()
                    .any(|seen| seen.name.to_lowercase() == property.name.to_lowercase())
            {
                if person {
                    sidecar.people.push(property.name.clone());
                }
                sidecar.properties.push(property);
            }
        }
    }
    if let Some(Value::Array(list)) = root.get("pages") {
        sidecar.pages = list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
    }
    Some(sidecar)
}

/// One property of the schema, and whether the sidecar declared it a `person`.
fn property(entry: &Value) -> Option<(PlannedProperty, bool)> {
    let name = entry.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let name = if name.eq_ignore_ascii_case(TAGS_NAME) {
        TAGS_NAME.to_owned()
    } else {
        truncate_chars(name, MAX_NAME_CHARS)
    };
    let declared = PropertyKind::parse(entry.get("kind")?.as_str()?)?;
    // A Person property holds accounts of the space (ADR-0049). An export lists the names of its
    // people, not their ids, and an account is not something an import can make up: the names are
    // text (the front matter's list of names is joined by commas).
    let person = declared == PropertyKind::Person;
    let kind = if person { PropertyKind::Text } else { declared };
    let options = match entry.get("options") {
        Some(Value::Array(list)) if kind.has_options() => {
            list.iter().filter_map(option).take(MAX_OPTIONS).collect()
        }
        _ => Vec::new(),
    };
    let config = match kind {
        PropertyKind::Relation => Some(relation_config(entry.get("config"))),
        PropertyKind::Rollup => Some(rollup_config(entry.get("config")?)?),
        PropertyKind::Formula => Some(formula_config(entry.get("config")?)?),
        _ => None,
    };
    Some((
        PlannedProperty {
            name,
            kind,
            options,
            config,
        },
        person,
    ))
}

fn option(entry: &Value) -> Option<PlannedOption> {
    let name = entry.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(PlannedOption {
        name: truncate_chars(name, MAX_NAME_CHARS),
        color: entry
            .get("color")
            .and_then(Value::as_str)
            .and_then(OptionColor::parse),
        group: entry
            .get("group")
            .and_then(Value::as_str)
            .and_then(StatusGroup::parse),
    })
}

fn relation_config(config: Option<&Value>) -> PlannedConfig {
    let two_way = config
        .and_then(|c| c.get("twoWay"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let pair_name = config
        .and_then(|c| c.get("pairName"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| truncate_chars(name, MAX_NAME_CHARS));
    PlannedConfig::Relation {
        two_way,
        pair_name: if two_way { pair_name } else { None },
    }
}

/// A formula's expression, in display form: the text is kept as written (names, spaces, comments),
/// the core resolves the names when the property is made and keeps a text that does not resolve.
fn formula_config(config: &Value) -> Option<PlannedConfig> {
    let expression = config.get("expression")?.as_str()?;
    if expression.trim().is_empty() {
        return None;
    }
    Some(PlannedConfig::Formula {
        expression: expression.to_owned(),
    })
}

fn rollup_config(config: &Value) -> Option<PlannedConfig> {
    let relation = config.get("relation")?.as_str()?.trim();
    if relation.is_empty() {
        return None;
    }
    let function = config.get("function")?.as_str()?;
    let function = RollupFunction::ALL
        .into_iter()
        .find(|candidate| candidate.as_str() == function)?;
    let target = match config.get("target")? {
        Value::String(text) if text == "title" => PlannedRollupTarget::Title,
        Value::String(name) if !name.trim().is_empty() => {
            PlannedRollupTarget::Property(name.trim().to_owned())
        }
        Value::Object(object) => {
            if let Some(name) = object.get("property").and_then(Value::as_str) {
                PlannedRollupTarget::Property(name.trim().to_owned())
            } else if object.get("title").is_some()
                || object.get("kind").and_then(Value::as_str) == Some("title")
            {
                PlannedRollupTarget::Title
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some(PlannedConfig::Rollup {
        relation: relation.to_owned(),
        target,
        function,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_documented_sidecar_is_read() {
        let text = json!({
            "format": 1,
            "generator": "atomik-kb",
            "name": "Projects",
            "exportedAt": "2026-10-12T09:30:00Z",
            "properties": [
                {"name": "tags", "kind": "multi_select", "options": [{"name": "work", "color": "blue"}]},
                {"name": "Status", "kind": "status", "options": [
                    {"name": "Done", "color": "green", "group": "done"},
                    {"name": "Odd", "color": "mauve", "group": "later"}
                ]},
                {"name": "Related", "kind": "relation", "config": {"twoWay": true, "pairName": "Related (back)"}},
                {"name": "Estimate", "kind": "number"},
                {"name": "Total", "kind": "rollup", "config": {"relation": "Related", "target": {"property": "Estimate"}, "function": "sum"}},
                {"name": "Count", "kind": "rollup", "config": {"relation": "Related", "target": "title", "function": "count_all"}},
                {"name": "Created", "kind": "created_time"}
            ],
            "pages": ["Projects.md", "Projects/Alpha.md"]
        })
        .to_string();
        let sidecar = parse(&text).unwrap();
        assert_eq!(sidecar.pages, ["Projects.md", "Projects/Alpha.md"]);
        let names: Vec<_> = sidecar.properties.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "Tags", "Status", "Related", "Estimate", "Total", "Count", "Created"
            ]
        );
        assert_eq!(sidecar.properties[0].kind, PropertyKind::MultiSelect);
        assert_eq!(
            sidecar.properties[0].options,
            [PlannedOption {
                name: "work".into(),
                color: Some(OptionColor::Blue),
                group: None
            }]
        );
        assert_eq!(
            sidecar.properties[1].options,
            [
                PlannedOption {
                    name: "Done".into(),
                    color: Some(OptionColor::Green),
                    group: Some(StatusGroup::Done)
                },
                PlannedOption {
                    name: "Odd".into(),
                    color: None,
                    group: None
                },
            ]
        );
        assert_eq!(
            sidecar.properties[2].config,
            Some(PlannedConfig::Relation {
                two_way: true,
                pair_name: Some("Related (back)".into())
            })
        );
        assert_eq!(
            sidecar.properties[4].config,
            Some(PlannedConfig::Rollup {
                relation: "Related".into(),
                target: PlannedRollupTarget::Property("Estimate".into()),
                function: RollupFunction::Sum,
            })
        );
        assert_eq!(
            sidecar.properties[5].config,
            Some(PlannedConfig::Rollup {
                relation: "Related".into(),
                target: PlannedRollupTarget::Title,
                function: RollupFunction::CountAll,
            })
        );
        assert_eq!(sidecar.properties[6].kind, PropertyKind::CreatedTime);
    }

    #[test]
    fn a_formula_keeps_its_expression_as_written() {
        let text = json!({
            "properties": [
                {"name": "Price", "kind": "number"},
                {"name": "Double", "kind": "formula", "config": {"expression": "prop(\"Price\") * 2 /* twice */"}},
                {"name": "NoConfig", "kind": "formula"},
                {"name": "Blank", "kind": "formula", "config": {"expression": "  "}},
                {"name": "Odd", "kind": "formula", "config": {"expression": 3}}
            ]
        })
        .to_string();
        let sidecar = parse(&text).unwrap();
        let names: Vec<_> = sidecar.properties.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            ["Price", "Double"],
            "formulas without a text are left out"
        );
        assert_eq!(sidecar.properties[1].kind, PropertyKind::Formula);
        assert_eq!(
            sidecar.properties[1].config,
            Some(PlannedConfig::Formula {
                expression: "prop(\"Price\") * 2 /* twice */".into()
            })
        );
    }

    #[test]
    fn a_person_property_is_planned_as_text_and_named() {
        let text = json!({
            "properties": [
                {"name": "Owner", "kind": "person"},
                {"name": "Estimate", "kind": "number"},
                {"name": "owner", "kind": "text"},
                {"name": "Reviewer", "kind": "person", "options": [{"name": "Ann"}], "config": {"twoWay": true}},
                {"name": "Notes", "kind": "text"}
            ]
        })
        .to_string();
        let sidecar = parse(&text).unwrap();
        let planned: Vec<_> = sidecar
            .properties
            .iter()
            .map(|p| (p.name.as_str(), p.kind))
            .collect();
        assert_eq!(
            planned,
            [
                ("Owner", PropertyKind::Text),
                ("Estimate", PropertyKind::Number),
                ("Reviewer", PropertyKind::Text),
                ("Notes", PropertyKind::Text),
            ]
        );
        // A text property has no options and no configuration, whatever the entry said.
        assert!(sidecar.properties[2].options.is_empty());
        assert_eq!(sidecar.properties[2].config, None);
        // Only the person properties that were kept are named: not the duplicate, not the texts.
        assert_eq!(sidecar.people, ["Owner", "Reviewer"]);
    }

    #[test]
    fn unreadable_entries_are_left_out() {
        let text = json!({
            "properties": [
                {"kind": "text"},
                {"name": "", "kind": "text"},
                {"name": "A", "kind": "no_such_kind"},
                {"name": "B", "kind": "rollup"},
                {"name": "C", "kind": "rollup", "config": {"relation": "R", "target": "title", "function": "nope"}},
                {"name": "D", "kind": "text"},
                {"name": "d", "kind": "number"},
                "junk"
            ],
            "pages": [1, "x.md"]
        })
        .to_string();
        let sidecar = parse(&text).unwrap();
        assert_eq!(sidecar.properties.len(), 1);
        assert_eq!(sidecar.properties[0].name, "D");
        assert_eq!(sidecar.pages, ["x.md"]);
        assert!(parse("[]").is_none());
        assert!(parse("not json").is_none());
        assert!(parse("{}").unwrap().properties.is_empty());
    }
}
