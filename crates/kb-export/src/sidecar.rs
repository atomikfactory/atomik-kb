//! The `atomik-export.json` sidecar of a Markdown archive (`docs/architecture/import-export.md`,
//! "Sidecar"): the property schema with names instead of ids and the pages in tree order, so a
//! re-import restores kinds, options, colours, relations, rollups and sibling order.
//!
//! ```json
//! {
//!   "format": 1,
//!   "generator": "atomik-kb",
//!   "name": "<export name>",
//!   "exportedAt": "2026-10-12T09:30:00Z",
//!   "properties": [
//!     {"name": "Status", "kind": "status", "options": [{"name": "Done", "color": "green", "group": "done"}]},
//!     {"name": "Related", "kind": "relation", "config": {"twoWay": true, "pairName": "Related (back)"}},
//!     {"name": "Total", "kind": "rollup", "config": {"relation": "Related", "target": {"property": "Estimate"}, "function": "sum"}}
//!   ],
//!   "pages": ["Projects.md", "Projects/Alpha.md"]
//! }
//! ```
//!
//! Keys are written in this order. `options` is there for the three kinds that have a list (an
//! empty one included), `group` for status options only. A relation's `config` is
//! `{"twoWay": false}` when it is one-way, and `pairName` (the paired property's name) is left out
//! when the pair is not in the schema. A rollup's `target` is `{"property": "<name>"}` or, for the
//! related pages' titles, `{"title": true}`; its `relation` is the relation property's name. A
//! rollup whose configuration is damaged (or whose relation or target is not in the schema) has no
//! `config`. Computed kinds (created and last edited time and by) are listed by kind alone, and so
//! is a Person property (`{"name": "Owner", "kind": "person"}`, ADR-0049): no member and no account
//! id is written, and a re-import reads it as text. A formula's `config` is `{"expression": "<text>"}`
//! in display form (ADR-0055): the properties it reads by name, `prop("Price")`, the title as
//! `prop("title")`; a re-import resolves the names in the target space.

use std::collections::HashMap;

use kb_model::formula;
use kb_types::{PropertyConfig, PropertyDef, PropertyId, PropertySchema, RollupTarget};

use crate::json::Node;
use crate::time::rfc3339_utc;

/// File name of the sidecar at the archive's top level.
pub const SIDECAR_NAME: &str = "atomik-export.json";

/// Version of the sidecar's layout.
const FORMAT: u32 = 1;

/// The sidecar's text (pretty JSON, ending with a newline). `page_files` are the archive paths of
/// the pages in tree order.
#[must_use]
pub fn sidecar(
    export_name: &str,
    exported_at_ms: i64,
    schema: &PropertySchema,
    page_files: &[String],
) -> String {
    let names: HashMap<PropertyId, &str> = schema
        .properties
        .iter()
        .map(|def| (def.id, def.name.as_str()))
        .collect();
    let document = Node::object([
        ("format", Node::Scalar(FORMAT.into())),
        ("generator", Node::string("atomik-kb")),
        ("name", Node::string(export_name)),
        ("exportedAt", Node::string(rfc3339_utc(exported_at_ms))),
        (
            "properties",
            Node::Array(
                schema
                    .properties
                    .iter()
                    .map(|def| property(def, &names))
                    .collect(),
            ),
        ),
        (
            "pages",
            Node::Array(page_files.iter().map(Node::string).collect()),
        ),
    ]);
    let mut text = document.pretty();
    text.push('\n');
    text
}

/// One property of the schema, by names.
fn property(def: &PropertyDef, names: &HashMap<PropertyId, &str>) -> Node {
    let mut pairs = vec![
        ("name".to_owned(), Node::string(def.name.as_str())),
        ("kind".to_owned(), Node::string(def.kind.as_str())),
    ];
    if def.kind.has_options() {
        let options = def
            .options
            .iter()
            .map(|option| {
                let mut fields = vec![
                    ("name".to_owned(), Node::string(option.name.as_str())),
                    ("color".to_owned(), Node::string(option.color.as_str())),
                ];
                if let Some(group) = option.group {
                    fields.push(("group".to_owned(), Node::string(group.as_str())));
                }
                Node::Object(fields)
            })
            .collect();
        pairs.push(("options".to_owned(), Node::Array(options)));
    }
    if let Some(config) = def
        .config
        .as_ref()
        .and_then(|config| config_node(config, names))
    {
        pairs.push(("config".to_owned(), config));
    }
    Node::Object(pairs)
}

/// The configuration of a relation, a rollup or a formula by names; `None` when a name it needs is
/// missing.
fn config_node(config: &PropertyConfig, names: &HashMap<PropertyId, &str>) -> Option<Node> {
    match config {
        PropertyConfig::Relation {
            two_way,
            paired_property_id,
        } => {
            let mut pairs = vec![("twoWay".to_owned(), Node::Scalar((*two_way).into()))];
            if *two_way && let Some(pair) = paired_property_id.and_then(|id| names.get(&id)) {
                pairs.push(("pairName".to_owned(), Node::string(*pair)));
            }
            Some(Node::Object(pairs))
        }
        PropertyConfig::Rollup {
            relation_property_id,
            target,
            function,
        } => {
            let relation = names.get(relation_property_id)?;
            let target = match target {
                RollupTarget::Title => Node::object([("title", Node::Scalar(true.into()))]),
                RollupTarget::Property(id) => {
                    Node::object([("property", Node::string(*names.get(id)?))])
                }
            };
            Some(Node::object([
                ("relation", Node::string(*relation)),
                ("target", target),
                ("function", Node::string(function.as_str())),
            ]))
        }
        // The expression as a person reads it: the ids of `prop("...")` become property names (an id the
        // schema lacks stays as it is, and the formula stays broken on re-import).
        PropertyConfig::Formula { expression, .. } => {
            let name_of = |id: &str| {
                let id = id.parse::<PropertyId>().ok()?;
                names.get(&id).map(|name| (*name).to_owned())
            };
            Some(Node::object([(
                "expression",
                Node::string(formula::to_display(expression, &name_of)),
            )]))
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use kb_types::properties::TAGS_PROPERTY_ID;
    use kb_types::{
        OptionColor, OptionId, PropertyKind, PropertyOption, RollupFunction, StatusGroup,
    };
    use serde_json::{Value, json};

    use super::*;

    fn pid(n: u8) -> PropertyId {
        PropertyId::from_random_bytes([n; 16])
    }

    fn option(n: u8, name: &str, color: OptionColor, group: Option<StatusGroup>) -> PropertyOption {
        PropertyOption {
            id: OptionId::from_random_bytes([n; 16]),
            name: name.to_owned(),
            color,
            group,
        }
    }

    fn def(
        id: PropertyId,
        name: &str,
        kind: PropertyKind,
        options: Vec<PropertyOption>,
        config: Option<PropertyConfig>,
    ) -> PropertyDef {
        PropertyDef {
            id,
            name: name.to_owned(),
            kind,
            options,
            created_at: 5,
            config,
        }
    }

    /// The schema of the spec's example, with Tags, a number and a created time.
    fn schema() -> PropertySchema {
        PropertySchema {
            properties: vec![
                def(
                    TAGS_PROPERTY_ID,
                    "Tags",
                    PropertyKind::MultiSelect,
                    vec![
                        option(1, "work", OptionColor::Blue, None),
                        option(2, "review", OptionColor::Pink, None),
                    ],
                    None,
                ),
                def(
                    pid(10),
                    "Status",
                    PropertyKind::Status,
                    vec![
                        option(3, "Not started", OptionColor::Gray, Some(StatusGroup::Todo)),
                        option(
                            4,
                            "Working",
                            OptionColor::Blue,
                            Some(StatusGroup::InProgress),
                        ),
                        option(5, "Done", OptionColor::Green, Some(StatusGroup::Done)),
                    ],
                    None,
                ),
                def(pid(11), "Estimate", PropertyKind::Number, vec![], None),
                def(
                    pid(12),
                    "Related",
                    PropertyKind::Relation,
                    vec![],
                    Some(PropertyConfig::Relation {
                        two_way: true,
                        paired_property_id: Some(pid(13)),
                    }),
                ),
                def(
                    pid(13),
                    "Related (back)",
                    PropertyKind::Relation,
                    vec![],
                    Some(PropertyConfig::Relation {
                        two_way: true,
                        paired_property_id: Some(pid(12)),
                    }),
                ),
                def(
                    pid(14),
                    "Total",
                    PropertyKind::Rollup,
                    vec![],
                    Some(PropertyConfig::Rollup {
                        relation_property_id: pid(12),
                        target: RollupTarget::Property(pid(11)),
                        function: RollupFunction::Sum,
                    }),
                ),
                def(pid(15), "Created", PropertyKind::CreatedTime, vec![], None),
            ],
        }
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn the_sidecar_follows_the_documented_shape() {
        let text = sidecar(
            "Projects",
            1_760_261_400_000,
            &schema(),
            &["Projects.md".to_owned(), "Projects/Alpha.md".to_owned()],
        );
        assert!(text.ends_with("}\n"));
        let value = parse(&text);
        assert_eq!(
            value,
            json!({
                "format": 1,
                "generator": "atomik-kb",
                "name": "Projects",
                "exportedAt": "2025-10-12T09:30:00Z",
                "properties": [
                    { "name": "Tags", "kind": "multi_select", "options": [
                        { "name": "work", "color": "blue" },
                        { "name": "review", "color": "pink" }
                    ] },
                    { "name": "Status", "kind": "status", "options": [
                        { "name": "Not started", "color": "gray", "group": "todo" },
                        { "name": "Working", "color": "blue", "group": "in_progress" },
                        { "name": "Done", "color": "green", "group": "done" }
                    ] },
                    { "name": "Estimate", "kind": "number" },
                    { "name": "Related", "kind": "relation", "config": { "twoWay": true, "pairName": "Related (back)" } },
                    { "name": "Related (back)", "kind": "relation", "config": { "twoWay": true, "pairName": "Related" } },
                    { "name": "Total", "kind": "rollup", "config": { "relation": "Related", "target": { "property": "Estimate" }, "function": "sum" } },
                    { "name": "Created", "kind": "created_time" }
                ],
                "pages": ["Projects.md", "Projects/Alpha.md"]
            })
        );
    }

    #[test]
    fn keys_are_written_in_the_documented_order() {
        let text = sidecar("X", 0, &schema(), &[]);
        let position = |needle: &str| text.find(needle).unwrap();
        assert!(position("\"format\"") < position("\"generator\""));
        assert!(position("\"generator\"") < position("\"name\""));
        assert!(position("\"name\": \"X\"") < position("\"exportedAt\""));
        assert!(position("\"exportedAt\"") < position("\"properties\""));
        assert!(position("\"properties\"") < position("\"pages\""));
        // The layout of serde_json's pretty printer.
        assert!(text.starts_with(
            "{\n  \"format\": 1,\n  \"generator\": \"atomik-kb\",\n  \"name\": \"X\",\n"
        ));
        assert!(text.contains("\"pages\": []\n}"));
        let status = position("\"name\": \"Status\"");
        let kind = position("\"kind\": \"status\"");
        assert!(status < kind);
        assert!(kind < status + text[status..].find("\"options\"").unwrap());
        // A relation's configuration comes after its kind.
        let related = position("\"name\": \"Related\"");
        assert!(
            related
                < position(
                    "\"config\": {
        \"twoWay\""
                )
        );
    }

    #[test]
    fn ids_never_appear() {
        let text = sidecar("X", 0, &schema(), &[]);
        for def in &schema().properties {
            assert!(!text.contains(&def.id.to_string()), "{}", def.name);
            for option in &def.options {
                assert!(!text.contains(&option.id.to_string()));
            }
        }
    }

    #[test]
    fn a_one_way_relation_and_a_lost_pair() {
        let schema = PropertySchema {
            properties: vec![
                def(
                    pid(1),
                    "Links",
                    PropertyKind::Relation,
                    vec![],
                    Some(PropertyConfig::Relation {
                        two_way: false,
                        paired_property_id: None,
                    }),
                ),
                def(
                    pid(2),
                    "Orphan",
                    PropertyKind::Relation,
                    vec![],
                    Some(PropertyConfig::Relation {
                        two_way: true,
                        paired_property_id: Some(pid(99)),
                    }),
                ),
            ],
        };
        let value = parse(&sidecar("X", 0, &schema, &[]));
        assert_eq!(value["properties"][0]["config"], json!({ "twoWay": false }));
        assert_eq!(value["properties"][1]["config"], json!({ "twoWay": true }));
    }

    #[test]
    fn rollups_name_their_relation_and_target() {
        let rollup = |relation: PropertyId, target: RollupTarget| {
            def(
                pid(9),
                "R",
                PropertyKind::Rollup,
                vec![],
                Some(PropertyConfig::Rollup {
                    relation_property_id: relation,
                    target,
                    function: RollupFunction::CountAll,
                }),
            )
        };
        let related = def(
            pid(1),
            "Related",
            PropertyKind::Relation,
            vec![],
            Some(PropertyConfig::Relation {
                two_way: false,
                paired_property_id: None,
            }),
        );
        let title = PropertySchema {
            properties: vec![related.clone(), rollup(pid(1), RollupTarget::Title)],
        };
        assert_eq!(
            parse(&sidecar("X", 0, &title, &[]))["properties"][1]["config"],
            json!({ "relation": "Related", "target": { "title": true }, "function": "count_all" })
        );
        // A relation or a target that is not in the schema: no configuration at all.
        let broken = PropertySchema {
            properties: vec![
                related.clone(),
                rollup(pid(98), RollupTarget::Title),
                rollup(pid(1), RollupTarget::Property(pid(99))),
            ],
        };
        let value = parse(&sidecar("X", 0, &broken, &[]));
        assert_eq!(
            value["properties"][1],
            json!({ "name": "R", "kind": "rollup" })
        );
        assert_eq!(
            value["properties"][2],
            json!({ "name": "R", "kind": "rollup" })
        );
    }

    #[test]
    fn a_formula_is_written_in_display_form_with_names_for_ids() {
        let formula = |expression: String| {
            def(
                pid(9),
                "Double",
                PropertyKind::Formula,
                vec![],
                Some(PropertyConfig::Formula {
                    expression,
                    result_type: None,
                }),
            )
        };
        let schema = PropertySchema {
            properties: vec![
                def(pid(1), "Price", PropertyKind::Number, vec![], None),
                def(pid(2), "Qty \"units\"", PropertyKind::Number, vec![], None),
                formula(format!(
                    "prop(\"{}\") * prop(\"{}\") + length(prop(\"title\")) /* total */",
                    pid(1),
                    pid(2)
                )),
            ],
        };
        let value = parse(&sidecar("X", 0, &schema, &[]));
        assert_eq!(
            value["properties"][2],
            json!({
                "name": "Double",
                "kind": "formula",
                "config": { "expression": "prop(\"Price\") * prop(\"Qty \\\"units\\\"\") + length(prop(\"title\")) /* total */" }
            })
        );
        // An id the schema lacks stays as it is.
        let broken = PropertySchema {
            properties: vec![formula(format!("prop(\"{}\")", pid(77)))],
        };
        assert_eq!(
            parse(&sidecar("X", 0, &broken, &[]))["properties"][0]["config"]["expression"],
            format!("prop(\"{}\")", pid(77))
        );
    }

    #[test]
    fn an_empty_schema_and_hostile_names_are_valid_json() {
        let empty = parse(&sidecar("", 0, &PropertySchema { properties: vec![] }, &[]));
        assert_eq!(empty["properties"], json!([]));
        assert_eq!(empty["exportedAt"], "1970-01-01T00:00:00Z");
        let hostile = "na\"me\\\n</script>--\u{1}é";
        let schema = PropertySchema {
            properties: vec![def(
                pid(1),
                hostile,
                PropertyKind::Select,
                vec![option(1, hostile, OptionColor::Red, None)],
                None,
            )],
        };
        let value = parse(&sidecar(hostile, 0, &schema, &[hostile.to_owned()]));
        assert_eq!(value["name"], hostile);
        assert_eq!(value["properties"][0]["name"], hostile);
        assert_eq!(value["properties"][0]["options"][0]["name"], hostile);
        assert_eq!(value["pages"][0], hostile);
        // A select with no options still says it has a list.
        let none = PropertySchema {
            properties: vec![def(pid(2), "S", PropertyKind::Select, vec![], None)],
        };
        assert_eq!(
            parse(&sidecar("X", 0, &none, &[]))["properties"][0]["options"],
            json!([])
        );
    }
}
