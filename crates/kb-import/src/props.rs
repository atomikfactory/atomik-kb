//! The properties a plan needs: one per name over the whole source, with one kind each.

use std::collections::{HashMap, HashSet};

use kb_types::PropertyKind;
use kb_types::properties::TAGS_NAME;

use crate::frontmatter::MAX_NAME_CHARS;
use crate::paths::truncate_chars;
use crate::{ImportValue, PlannedConfig, PlannedOption, PlannedProperty};

/// Most options kept for one property.
const MAX_OPTIONS: usize = 500;

struct Agg {
    name: String,
    /// The kind seen so far (`None`: only empty observations).
    kind: Option<PropertyKind>,
    options: Vec<String>,
    seen: HashSet<String>,
    /// The sidecar's definition, which decides the kind.
    fixed: Option<PlannedProperty>,
}

/// Collects what the pages say about their properties.
#[derive(Default)]
pub(crate) struct Aggregator {
    by_name: HashMap<String, usize>,
    list: Vec<Agg>,
}

/// The kind a value implies.
pub(crate) fn kind_of_value(value: &ImportValue) -> PropertyKind {
    match value {
        ImportValue::Text(_) => PropertyKind::Text,
        ImportValue::Number(_) => PropertyKind::Number,
        ImportValue::Bool(_) => PropertyKind::Checkbox,
        ImportValue::Date { .. } => PropertyKind::Date,
        ImportValue::List(_) => PropertyKind::MultiSelect,
        ImportValue::Pages(_) => PropertyKind::Relation,
        ImportValue::Files(_) => PropertyKind::Files,
        ImportValue::Checklist(_) => PropertyKind::Checklist,
        ImportValue::Place { .. } => PropertyKind::Place,
    }
}

fn key(name: &str) -> String {
    name.to_lowercase()
}

/// The name a property is planned under: `Tags` for any spelling of it, else the name cut to the
/// schema's limit.
fn display_name(name: &str) -> String {
    let name = name.trim();
    if name.eq_ignore_ascii_case(TAGS_NAME) {
        TAGS_NAME.to_owned()
    } else {
        truncate_chars(name, MAX_NAME_CHARS)
    }
}

impl Aggregator {
    /// An aggregator that starts with the sidecar's schema.
    pub(crate) fn new(sidecar: &[PlannedProperty]) -> Self {
        let mut aggregator = Self::default();
        for property in sidecar {
            let index = aggregator.slot(&property.name);
            aggregator.list[index].fixed = Some(property.clone());
            aggregator.list[index].kind = Some(property.kind);
        }
        aggregator
    }

    fn slot(&mut self, name: &str) -> usize {
        let name = display_name(name);
        if let Some(index) = self.by_name.get(&key(&name)) {
            return *index;
        }
        let index = self.list.len();
        self.by_name.insert(key(&name), index);
        self.list.push(Agg {
            name,
            kind: None,
            options: Vec::new(),
            seen: HashSet::new(),
            fixed: None,
        });
        index
    }

    /// The name a property is planned under, for a name as a page wrote it.
    pub(crate) fn canonical(&self, name: &str) -> Option<&str> {
        self.by_name
            .get(&key(&display_name(name)))
            .map(|index| self.list[*index].name.as_str())
    }

    /// The final kind of a property, by name.
    pub(crate) fn kind_of(&self, name: &str) -> Option<PropertyKind> {
        let index = self.by_name.get(&key(&display_name(name)))?;
        let agg = &self.list[*index];
        Some(agg.fixed.as_ref().map_or_else(
            || agg.kind.unwrap_or(PropertyKind::Text),
            |fixed| fixed.kind,
        ))
    }

    /// A column or a value of a kind (`None`: empty), with the option names it holds.
    pub(crate) fn observe(&mut self, name: &str, kind: Option<PropertyKind>, options: &[String]) {
        if name.trim().is_empty() {
            return;
        }
        let index = self.slot(name);
        let agg = &mut self.list[index];
        if agg.fixed.is_none()
            && let Some(kind) = kind
        {
            agg.kind = Some(match agg.kind {
                None => kind,
                Some(seen) if seen == kind => kind,
                // Different kinds are text.
                Some(_) => PropertyKind::Text,
            });
        }
        for option in options {
            let option = truncate_chars(option.trim(), MAX_NAME_CHARS);
            if !option.is_empty()
                && agg.options.len() < MAX_OPTIONS
                && agg.seen.insert(option.to_lowercase())
            {
                agg.options.push(option);
            }
        }
    }

    /// A value of a page.
    pub(crate) fn observe_value(&mut self, name: &str, value: &ImportValue) {
        let options: Vec<String> = match value {
            ImportValue::List(items) => items.clone(),
            ImportValue::Text(text) => vec![text.clone()],
            _ => Vec::new(),
        };
        // Text is only an option for a property whose kind has options; `observe` keeps them and
        // `finish` drops them from the others.
        let observed_kind = Some(kind_of_value(value));
        let keep_options = matches!(value, ImportValue::List(_)) || self.has_option_kind(name);
        self.observe(
            name,
            observed_kind,
            if keep_options { &options } else { &[] },
        );
    }

    fn has_option_kind(&self, name: &str) -> bool {
        self.by_name
            .get(&key(&display_name(name)))
            .and_then(|index| self.list[*index].fixed.as_ref())
            .is_some_and(|fixed| fixed.kind.has_options())
    }

    /// The planned properties, the sidecar's first, then in order of appearance.
    pub(crate) fn finish(self) -> Vec<PlannedProperty> {
        self.list
            .into_iter()
            .map(|agg| match agg.fixed {
                Some(mut fixed) => {
                    if fixed.kind.has_options() {
                        let mut seen: HashSet<String> = fixed
                            .options
                            .iter()
                            .map(|o| o.name.to_lowercase())
                            .collect();
                        for option in agg.options {
                            if fixed.options.len() < MAX_OPTIONS
                                && seen.insert(option.to_lowercase())
                            {
                                fixed.options.push(PlannedOption {
                                    name: option,
                                    color: None,
                                    group: None,
                                });
                            }
                        }
                    }
                    fixed
                }
                None => {
                    let kind = agg.kind.unwrap_or(PropertyKind::Text);
                    let options = if kind.has_options() {
                        agg.options
                            .into_iter()
                            .map(|name| PlannedOption {
                                name,
                                color: None,
                                group: None,
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    let config =
                        (kind == PropertyKind::Relation).then_some(PlannedConfig::Relation {
                            two_way: false,
                            pair_name: None,
                        });
                    PlannedProperty {
                        name: agg.name,
                        kind,
                        options,
                        config,
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn one_property_per_name_with_one_kind() {
        let mut aggregator = Aggregator::default();
        aggregator.observe(
            "Status",
            Some(PropertyKind::Select),
            &["Open".into(), "Closed".into()],
        );
        aggregator.observe(
            "status",
            Some(PropertyKind::Select),
            &["open".into(), "Blocked".into()],
        );
        aggregator.observe("Estimate", Some(PropertyKind::Number), &[]);
        aggregator.observe("estimate", Some(PropertyKind::Text), &[]);
        aggregator.observe("Empty", None, &[]);
        aggregator.observe("Due", Some(PropertyKind::Date), &[]);
        aggregator.observe("Due", None, &[]);
        let got = aggregator.finish();
        let names: Vec<_> = got.iter().map(|p| (p.name.as_str(), p.kind)).collect();
        assert_eq!(
            names,
            [
                ("Status", PropertyKind::Select),
                ("Estimate", PropertyKind::Text),
                ("Empty", PropertyKind::Text),
                ("Due", PropertyKind::Date)
            ]
        );
        let options: Vec<_> = got[0].options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(options, ["Open", "Closed", "Blocked"]);
        assert!(got[1].options.is_empty());
    }

    #[test]
    fn values_imply_kinds_and_tags_are_one_name() {
        let mut aggregator = Aggregator::default();
        aggregator.observe_value("tags", &ImportValue::List(vec!["a".into(), "b".into()]));
        aggregator.observe_value("Tags", &ImportValue::List(vec!["b".into(), "c".into()]));
        aggregator.observe_value("Done", &ImportValue::Bool(true));
        aggregator.observe_value("Related", &ImportValue::Pages(Vec::new()));
        let got = aggregator.finish();
        assert_eq!(got[0].name, "Tags");
        assert_eq!(got[0].kind, PropertyKind::MultiSelect);
        assert_eq!(got[0].options.len(), 3);
        assert_eq!(got[1].kind, PropertyKind::Checkbox);
        assert_eq!(got[2].kind, PropertyKind::Relation);
        assert_eq!(
            got[2].config,
            Some(PlannedConfig::Relation {
                two_way: false,
                pair_name: None
            })
        );
    }

    #[test]
    fn the_sidecar_decides_and_gets_the_options_values_use() {
        let sidecar = vec![PlannedProperty {
            name: "Status".into(),
            kind: PropertyKind::Status,
            options: vec![PlannedOption {
                name: "Done".into(),
                color: Some(kb_types::OptionColor::Green),
                group: Some(kb_types::StatusGroup::Done),
            }],
            config: None,
        }];
        let mut aggregator = Aggregator::new(&sidecar);
        aggregator.observe_value("status", &ImportValue::Text("Done".into()));
        aggregator.observe_value("Status", &ImportValue::Text("Doing".into()));
        aggregator.observe_value("Status", &ImportValue::Number(3.0));
        assert_eq!(aggregator.canonical("STATUS"), Some("Status"));
        assert_eq!(aggregator.kind_of("status"), Some(PropertyKind::Status));
        let got = aggregator.finish();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].kind, PropertyKind::Status);
        let options: Vec<_> = got[0]
            .options
            .iter()
            .map(|o| (o.name.as_str(), o.color))
            .collect();
        assert_eq!(
            options,
            [
                ("Done", Some(kb_types::OptionColor::Green)),
                ("Doing", None)
            ]
        );
    }

    #[test]
    fn names_are_cut_and_blank_names_ignored() {
        let mut aggregator = Aggregator::default();
        aggregator.observe(&"n".repeat(300), Some(PropertyKind::Text), &[]);
        aggregator.observe("   ", Some(PropertyKind::Text), &[]);
        let got = aggregator.finish();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name.chars().count(), MAX_NAME_CHARS);
    }
}
