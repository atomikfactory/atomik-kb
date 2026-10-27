//! The properties of a moved subtree in the target space (`docs/architecture/space-move.md`,
//! section 4, steps 2 and 5): which target property each source property maps to (by name and
//! kind, created when missing), how each stored value follows, and how the views (`pageList`
//! blocks) of the moved pages are remapped. Pure functions over the two schemas: the plan request
//! and the move run the same code, so their counts agree.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use kb_model::formula;
use kb_model::placeholder_fits;
use kb_model::views::VIEWS_ATTR;
use kb_types::properties::{
    DEFAULT_STATUS_OPTIONS, MAX_NAME_CHARS, MAX_OPTIONS, MAX_PROPERTIES, MAX_SELECTED_OPTIONS,
    MAX_TEXT_CHARS, TAGS_PROPERTY_ID,
};
use kb_types::{
    AccountId, DateValue, OptionColor, OptionId, PageId, PlaceValue, PropertyConfig, PropertyDef,
    PropertyId, PropertyKind, PropertyOption, PropertyValue, Rng, RollupTarget, StatusGroup,
    TemplatePlaceholder,
};
use serde_json::{Map, Value};

/// What the properties of the moved pages and their views need from the target schema.
#[derive(Debug, Default)]
pub(crate) struct Needs {
    /// Source properties that need a counterpart in the target: those a moved page holds a
    /// value of, and those a view of a moved page refers to.
    pub(crate) props: BTreeSet<PropertyId>,
    /// Per source property, the options something refers to (a value, a filter, a board column).
    pub(crate) options: BTreeMap<PropertyId, BTreeSet<OptionId>>,
    /// The properties a view refers to (a computed property gets a counterpart only for these).
    pub(crate) views: BTreeSet<PropertyId>,
}

impl Needs {
    /// Notes the stored values of one moved page (raw JSON keyed by source property id).
    pub(crate) fn add_values(&mut self, src: &[PropertyDef], raw: &[(PropertyId, Value)]) {
        for (property, stored) in raw {
            let Some(def) = src.iter().find(|def| def.id == *property) else {
                continue;
            };
            if def.kind.is_computed() {
                continue;
            }
            let Some(value) = decode(def.kind, stored) else {
                continue;
            };
            self.props.insert(*property);
            if let Some(options) = options_of(&value) {
                let known: Vec<OptionId> = options
                    .into_iter()
                    .filter(|option| def.options.iter().any(|o| o.id == *option))
                    .collect();
                self.options.entry(*property).or_default().extend(known);
            }
        }
    }

    /// Notes the template placeholders of one moved page (ADR-0056, raw as the model stores them, by
    /// source property id): a placeholder is a kind of value, so the property it is on needs a
    /// counterpart in the target (found by name, or created) when the source property takes it.
    pub(crate) fn add_placeholders(
        &mut self,
        src: &[PropertyDef],
        entries: &[(PropertyId, TemplatePlaceholder)],
    ) {
        for (property, placeholder) in entries {
            if src
                .iter()
                .any(|def| def.id == *property && placeholder_fits(*placeholder, def.kind))
            {
                self.props.insert(*property);
            }
        }
    }

    /// Notes what one `pageList` block refers to: its own attributes (the mirror of its first view) and
    /// those of each of its views (ADR-0054), which are the same places plus the filter tree, `dateBy`
    /// and `endBy`.
    pub(crate) fn add_view(&mut self, attrs: &Map<String, Value>) {
        self.add_attrs(attrs, true);
        if let Some(Value::Array(views)) = attrs.get(VIEWS_ATTR) {
            for view in views {
                if let Value::Object(view) = view {
                    self.add_attrs(view, false);
                }
            }
        }
    }

    /// The properties the formulas read: a formula something needs (a view names it) needs the
    /// properties its expression reads too, and so on through formulas that read formulas
    /// (ADR-0055). A computed property it reads gets a counterpart like one a view names.
    pub(crate) fn add_formula_reads(&mut self, src: &[PropertyDef]) {
        loop {
            let before = self.props.len();
            let reads: Vec<PropertyId> = src
                .iter()
                .filter(|def| def.kind == PropertyKind::Formula && self.props.contains(&def.id))
                .flat_map(formula_reads)
                .filter(|read| src.iter().any(|def| def.id == *read))
                .collect();
            for read in reads {
                self.props.insert(read);
                self.views.insert(read);
            }
            if self.props.len() == before {
                break;
            }
        }
    }

    /// One filter: its property, and the options of a `hasOption` or `lacksOption` (ADR-0058).
    fn add_filter(&mut self, filter: &Value) {
        let Some(id) = filter.get("propertyId").and_then(property_id_of) else {
            return;
        };
        self.props.insert(id);
        self.views.insert(id);
        if takes_option_ids(filter.get("op")) {
            let options = option_ids_of(filter.get("value"));
            self.options.entry(id).or_default().extend(options);
        }
    }

    /// The filters of a stored filter group, nested groups included.
    fn add_group(&mut self, group: &Value, depth: usize) {
        let Some(Value::Array(items)) = group.get("items") else {
            return;
        };
        if depth > MAX_GROUP_DEPTH {
            return;
        }
        for item in items {
            if is_group(item) {
                self.add_group(item, depth + 1);
            } else {
                self.add_filter(item);
            }
        }
    }

    /// The attributes of a block (`is_block`) or of one of its views: a view is the block's attributes,
    /// one per view, with `filter` (a tree) next to the flat `filters`, plus `dateBy` and `endBy`; the
    /// `source` is the block's alone.
    fn add_attrs(&mut self, attrs: &Map<String, Value>, is_block: bool) {
        let prop = |needs: &mut Self, id: PropertyId| {
            needs.props.insert(id);
            needs.views.insert(id);
        };
        if let Some(Value::Array(ids)) = attrs.get("properties") {
            for id in ids.iter().filter_map(property_id_of) {
                prop(self, id);
            }
        }
        if let Some(by) = attrs
            .get("sort")
            .and_then(|sort| sort.get("by"))
            .and_then(property_id_of)
        {
            prop(self, by);
        }
        // The other keys of a view's `sorts` (ADR-0058).
        if let Some(Value::Array(keys)) = attrs.get("sorts") {
            for by in keys
                .iter()
                .filter_map(|key| key.get("by").and_then(property_id_of))
            {
                prop(self, by);
            }
        }
        if let Some(Value::Array(filters)) = attrs.get("filters") {
            for filter in filters {
                self.add_filter(filter);
            }
        }
        if let Some(group) = attrs.get("filter") {
            self.add_group(group, 0);
        }
        // The calendar's date properties (`created` and `updated` are the page's own times and
        // parse as no property id).
        for key in ["dateBy", "endBy"] {
            if let Some(id) = attrs.get(key).and_then(property_id_of) {
                prop(self, id);
            }
        }
        if let Some(group) = attrs.get("groupBy").and_then(property_id_of) {
            prop(self, group);
            if let Some(Value::Array(hidden)) = attrs.get("hiddenGroups") {
                let options: Vec<OptionId> = hidden.iter().filter_map(option_id_of).collect();
                self.options.entry(group).or_default().extend(options);
            }
        }
        if let Some(Value::Object(widths)) = attrs.get("columnWidths") {
            for key in widths.keys() {
                if let Ok(id) = key.parse::<PropertyId>() {
                    prop(self, id);
                }
            }
        }
        if let Some(Value::Object(calculations)) = attrs.get("calculations") {
            for key in calculations.keys() {
                if let Ok(id) = key.parse::<PropertyId>() {
                    prop(self, id);
                }
            }
        }
        // The widgets of a dashboard (ADR-0055): their own filters, the column of a number, the
        // grouping and the measure of a chart.
        if let Some(Value::Array(widgets)) = attrs.get("widgets") {
            for widget in widgets {
                if let Some(group) = widget.get("filter") {
                    self.add_group(group, 0);
                }
                let reads = [
                    widget.get("calc").and_then(|calc| calc.get("by")),
                    widget.get("chart").and_then(|chart| chart.get("groupBy")),
                    widget
                        .get("chart")
                        .and_then(|chart| chart.get("measure"))
                        .and_then(|measure| measure.get("propertyId")),
                ];
                for id in reads.into_iter().flatten().filter_map(property_id_of) {
                    prop(self, id);
                }
            }
        }
        if is_block
            && let Some(source) = attrs.get("source")
            && source.get("kind").and_then(Value::as_str) == Some("tag")
            && let Some(option) = source.get("optionId").and_then(option_id_of)
        {
            prop(self, TAGS_PROPERTY_ID);
            self.options
                .entry(TAGS_PROPERTY_ID)
                .or_default()
                .insert(option);
        }
    }
}

/// One change to the target schema, in the order to apply them.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SchemaOp {
    /// A new property. A relation is one-way, a rollup carries its configuration, a status
    /// property starts with the three default options whose ids are `spare_options`.
    Property {
        id: PropertyId,
        name: String,
        kind: PropertyKind,
        config: Option<PropertyConfig>,
        spare_options: Vec<OptionId>,
    },
    /// A two-way relation: two properties made together.
    RelationPair {
        forward: PropertyId,
        back: PropertyId,
        name: String,
        back_name: String,
    },
    /// An option added to a select, multi-select or status property.
    AddOption {
        property: PropertyId,
        id: OptionId,
        name: String,
        color: OptionColor,
        group: Option<StatusGroup>,
    },
    /// A default option of a new status property that nothing maps to.
    DeleteOption {
        property: PropertyId,
        option: OptionId,
    },
}

/// How the source schema maps to the target schema.
#[derive(Debug, Default)]
pub(crate) struct SchemaMapping {
    /// The changes to make to the target schema, in order.
    pub(crate) ops: Vec<SchemaOp>,
    /// Source property -> target property (absent: no counterpart, its values cannot follow).
    pub(crate) props: HashMap<PropertyId, PropertyId>,
    /// Per source property: source option -> target option.
    pub(crate) options: HashMap<PropertyId, HashMap<OptionId, OptionId>>,
    /// Properties created in the target (a pair counts for two).
    pub(crate) created: u32,
    /// The target schema once the changes are made (what the values are mapped against).
    pub(crate) target: Vec<PropertyDef>,
}

/// Maps the source schema to the target's for the properties in `needs`, creating what is
/// missing (virtually: [`SchemaMapping::target`] is the schema after the ops, which the caller
/// replays on the real document). `src` and `dst` are `PropertySchemaGet` answers (Tags first).
pub(crate) fn plan_schema(
    src: &[PropertyDef],
    dst: &[PropertyDef],
    needs: &Needs,
    rng: &dyn Rng,
) -> SchemaMapping {
    let mut planner = Planner {
        src,
        needs,
        rng,
        target: dst.to_vec(),
        ops: Vec::new(),
        props: HashMap::new(),
        options: HashMap::new(),
        created: 0,
    };
    for def in src {
        if !needs.props.contains(&def.id) {
            continue;
        }
        match def.kind {
            // After the others: a rollup needs its relation's counterpart, a formula the
            // counterparts of what it reads.
            PropertyKind::Rollup | PropertyKind::Formula => {}
            PropertyKind::Relation => planner.relation(def),
            _ => planner.plain(def),
        }
    }
    for def in src {
        if def.kind == PropertyKind::Rollup {
            planner.rollup(def);
        }
    }
    planner.formulas();
    SchemaMapping {
        ops: planner.ops,
        props: planner.props,
        options: planner.options,
        created: planner.created,
        target: planner.target,
    }
}

/// The template placeholders of a moved page in the target schema (ADR-0056): each one follows its
/// property to the counterpart the mapping found, and is dropped when there is none or when the
/// counterpart's kind does not take it. A target property that two source properties map to keeps
/// the placeholder of the first.
pub(crate) fn map_placeholders(
    mapping: &SchemaMapping,
    entries: &[(PropertyId, TemplatePlaceholder)],
) -> Vec<(PropertyId, TemplatePlaceholder)> {
    let mut out: Vec<(PropertyId, TemplatePlaceholder)> = Vec::new();
    for (property, placeholder) in entries {
        let Some(target) = mapping.props.get(property).copied() else {
            continue;
        };
        let fits = mapping
            .target
            .iter()
            .find(|def| def.id == target)
            .is_some_and(|def| placeholder_fits(*placeholder, def.kind));
        if fits && !out.iter().any(|(known, _)| *known == target) {
            out.push((target, *placeholder));
        }
    }
    out.sort_unstable_by_key(|(property, _)| *property);
    out
}

/// Names compare by Unicode lower-casing of the trimmed text.
fn fold(name: &str) -> String {
    name.trim().chars().flat_map(char::to_lowercase).collect()
}

/// The properties a formula reads: the ids of its `prop` calls that are property ids.
fn formula_reads(def: &PropertyDef) -> Vec<PropertyId> {
    match &def.config {
        Some(PropertyConfig::Formula { expression, .. }) => formula::referenced_ids(expression)
            .iter()
            .filter_map(|read| read.parse::<PropertyId>().ok())
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether the property kinds can share a property: the same kind, or both pick options by name.
fn compatible(a: PropertyKind, b: PropertyKind) -> bool {
    a == b || (a.has_options() && b.has_options())
}

/// A property or option name the model accepts: not blank, at most 100 characters, no control
/// characters.
fn valid_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.chars().any(char::is_control)
}

struct Planner<'a> {
    src: &'a [PropertyDef],
    needs: &'a Needs,
    rng: &'a dyn Rng,
    /// The target schema as it will be after the ops.
    target: Vec<PropertyDef>,
    ops: Vec<SchemaOp>,
    props: HashMap<PropertyId, PropertyId>,
    options: HashMap<PropertyId, HashMap<OptionId, OptionId>>,
    created: u32,
}

impl Planner<'_> {
    fn index_of(&self, id: PropertyId) -> Option<usize> {
        self.target.iter().position(|def| def.id == id)
    }

    /// A target property with the name of `def`: one of a compatible kind first; any kind when
    /// `strict` is off (its values then go through their text form).
    fn find_by_name(&self, def: &PropertyDef, strict: bool) -> Option<usize> {
        let name = fold(&def.name);
        let mut other_kind = None;
        for (at, candidate) in self.target.iter().enumerate() {
            if fold(&candidate.name) != name {
                continue;
            }
            if compatible(def.kind, candidate.kind) {
                return Some(at);
            }
            if !strict && other_kind.is_none() {
                other_kind = Some(at);
            }
        }
        other_kind
    }

    /// Any property but a relation, a rollup or a computed one needing a view.
    fn plain(&mut self, def: &PropertyDef) {
        if def.id == TAGS_PROPERTY_ID {
            if let Some(at) = self.index_of(TAGS_PROPERTY_ID) {
                self.bind(def, at);
            }
            return;
        }
        if def.kind.is_computed() {
            match self.find_by_name(def, true) {
                Some(at) => self.bind(def, at),
                None if self.needs.views.contains(&def.id) => {
                    self.create(def);
                }
                None => {}
            }
            return;
        }
        match self.find_by_name(def, false) {
            Some(at) => self.bind(def, at),
            None => {
                self.create(def);
            }
        }
    }

    /// A relation: the target's relation of that name, else a new one: a pair when the source
    /// relation is two-way, its partner is used too and the target has neither.
    fn relation(&mut self, def: &PropertyDef) {
        if self.props.contains_key(&def.id) {
            return;
        }
        if let Some(at) = self.relation_by_name(def) {
            self.props.insert(def.id, self.target[at].id);
            return;
        }
        let partner = match &def.config {
            Some(PropertyConfig::Relation {
                two_way: true,
                paired_property_id: Some(partner),
            }) => self
                .src
                .iter()
                .find(|other| other.id == *partner && other.kind == PropertyKind::Relation),
            _ => None,
        };
        if let Some(partner) = partner
            && self.needs.props.contains(&partner.id)
            && !self.props.contains_key(&partner.id)
            && self.relation_by_name(partner).is_none()
            && self.target.len() + 2 <= MAX_PROPERTIES
            && valid_name(&def.name)
            && valid_name(&partner.name)
        {
            let forward = PropertyId::generate(self.rng);
            let back = PropertyId::generate(self.rng);
            self.ops.push(SchemaOp::RelationPair {
                forward,
                back,
                name: def.name.clone(),
                back_name: partner.name.clone(),
            });
            for (id, name, paired) in [(forward, &def.name, back), (back, &partner.name, forward)] {
                self.target.push(PropertyDef {
                    id,
                    name: name.clone(),
                    kind: PropertyKind::Relation,
                    options: Vec::new(),
                    created_at: 0,
                    config: Some(PropertyConfig::Relation {
                        two_way: true,
                        paired_property_id: Some(paired),
                    }),
                });
            }
            self.created += 2;
            self.props.insert(def.id, forward);
            self.props.insert(partner.id, back);
            return;
        }
        self.create(def);
    }

    fn relation_by_name(&self, def: &PropertyDef) -> Option<usize> {
        let name = fold(&def.name);
        self.target.iter().position(|candidate| {
            candidate.kind == PropertyKind::Relation && fold(&candidate.name) == name
        })
    }

    /// A rollup: the target's rollup of that name, else a new one when its relation (and its
    /// target property) have counterparts and it is a view's or its relation was just created.
    fn rollup(&mut self, def: &PropertyDef) {
        if let Some(at) = self.find_by_name(def, true) {
            self.props.insert(def.id, self.target[at].id);
            return;
        }
        let Some(PropertyConfig::Rollup {
            relation_property_id,
            target,
            function,
        }) = &def.config
        else {
            return;
        };
        let Some(&relation) = self.props.get(relation_property_id) else {
            return;
        };
        let relation_is_new = self.ops.iter().any(|op| match op {
            SchemaOp::Property { id, .. } => *id == relation,
            SchemaOp::RelationPair { forward, back, .. } => {
                *forward == relation || *back == relation
            }
            _ => false,
        });
        if !(relation_is_new || self.needs.views.contains(&def.id)) {
            return;
        }
        let (target, target_kind) = match target {
            RollupTarget::Title => (RollupTarget::Title, None),
            RollupTarget::Property(property) => {
                let Some(&mapped) = self.props.get(property) else {
                    return;
                };
                let Some(at) = self.index_of(mapped) else {
                    return;
                };
                (RollupTarget::Property(mapped), Some(self.target[at].kind))
            }
        };
        if !function.accepts(target_kind) {
            return;
        }
        let config = PropertyConfig::Rollup {
            relation_property_id: relation,
            target,
            function: *function,
        };
        self.create_with(def, Some(config));
    }

    /// The formulas something needs, each after the formulas it reads (a loop, or a formula that
    /// reads itself, is made last; it stays broken in the target).
    fn formulas(&mut self) {
        let (src, needs) = (self.src, self.needs);
        let mut pending: Vec<&PropertyDef> = src
            .iter()
            .filter(|def| def.kind == PropertyKind::Formula && needs.props.contains(&def.id))
            .collect();
        while !pending.is_empty() {
            let waiting: HashSet<PropertyId> = pending.iter().map(|def| def.id).collect();
            let (ready, rest): (Vec<&PropertyDef>, Vec<&PropertyDef>) =
                pending.into_iter().partition(|def| {
                    formula_reads(def)
                        .iter()
                        .all(|read| *read == def.id || !waiting.contains(read))
                });
            if ready.is_empty() {
                for def in rest {
                    self.formula(def);
                }
                break;
            }
            for def in ready {
                self.formula(def);
            }
            pending = rest;
        }
    }

    /// A formula: the target's formula of that name, else a new one whose expression reads the
    /// counterparts (an id with no counterpart stays as it is: the formula is then broken in the
    /// target, has no value, and its settings show the error).
    fn formula(&mut self, def: &PropertyDef) {
        if let Some(at) = self.find_by_name(def, true) {
            self.props.insert(def.id, self.target[at].id);
            return;
        }
        let Some(PropertyConfig::Formula { expression, .. }) = &def.config else {
            return;
        };
        let expression = formula::remap_ids(expression, &|id| {
            let source = id.parse::<PropertyId>().ok()?;
            self.props.get(&source).map(ToString::to_string)
        });
        self.create_with(
            def,
            Some(PropertyConfig::Formula {
                expression,
                result_type: None,
            }),
        );
    }

    /// Records that `def` uses the existing target property at `at`.
    fn bind(&mut self, def: &PropertyDef, at: usize) {
        self.props.insert(def.id, self.target[at].id);
        if def.kind.has_options() && self.target[at].kind.has_options() {
            self.map_options(def, at, false);
        }
    }

    fn create(&mut self, def: &PropertyDef) -> Option<usize> {
        self.create_with(def, None)
    }

    /// Adds a property like `def` to the target schema (virtually, and as an op), with all its
    /// options when it has some. `None` when the schema is full or the name is not acceptable.
    fn create_with(&mut self, def: &PropertyDef, config: Option<PropertyConfig>) -> Option<usize> {
        if self.target.len() >= MAX_PROPERTIES || !valid_name(&def.name) {
            return None;
        }
        let id = PropertyId::generate(self.rng);
        let spare: Vec<OptionId> = if def.kind == PropertyKind::Status {
            DEFAULT_STATUS_OPTIONS
                .iter()
                .map(|_| OptionId::generate(self.rng))
                .collect()
        } else {
            Vec::new()
        };
        let defaults: Vec<PropertyOption> = if def.kind == PropertyKind::Status {
            DEFAULT_STATUS_OPTIONS
                .iter()
                .zip(&spare)
                .map(|((name, group, color), id)| PropertyOption {
                    id: *id,
                    name: (*name).to_owned(),
                    color: *color,
                    group: Some(*group),
                })
                .collect()
        } else {
            Vec::new()
        };
        let virtual_config = match def.kind {
            PropertyKind::Relation => Some(PropertyConfig::Relation {
                two_way: false,
                paired_property_id: None,
            }),
            _ => config.clone(),
        };
        self.ops.push(SchemaOp::Property {
            id,
            name: def.name.clone(),
            kind: def.kind,
            config,
            spare_options: spare,
        });
        self.target.push(PropertyDef {
            id,
            name: def.name.clone(),
            kind: def.kind,
            options: defaults.clone(),
            created_at: 0,
            config: virtual_config,
        });
        self.created += 1;
        self.props.insert(def.id, id);
        let at = self.target.len() - 1;
        if def.kind.has_options() {
            self.map_options(def, at, true);
            // The default options of a new status property that nothing maps to go again.
            if def.kind == PropertyKind::Status {
                let mapped: HashSet<OptionId> = self
                    .options
                    .get(&def.id)
                    .map(|map| map.values().copied().collect())
                    .unwrap_or_default();
                for option in defaults {
                    if !mapped.contains(&option.id) {
                        self.ops.push(SchemaOp::DeleteOption {
                            property: id,
                            option: option.id,
                        });
                        self.target[at].options.retain(|o| o.id != option.id);
                    }
                }
            }
        }
        Some(at)
    }

    /// Maps the options of `def` to those of the target property at `at` by name, adding the
    /// ones the target lacks: every option of a new property (`all`), the ones something refers
    /// to otherwise.
    fn map_options(&mut self, def: &PropertyDef, at: usize, all: bool) {
        let wanted: Vec<PropertyOption> = def
            .options
            .iter()
            .filter(|option| {
                all || self
                    .needs
                    .options
                    .get(&def.id)
                    .is_some_and(|set| set.contains(&option.id))
            })
            .cloned()
            .collect();
        let target_id = self.target[at].id;
        let target_is_status = self.target[at].kind == PropertyKind::Status;
        let mut map = self.options.remove(&def.id).unwrap_or_default();
        for option in wanted {
            let wanted_name = fold(&option.name);
            let found = self.target[at]
                .options
                .iter()
                .find(|candidate| fold(&candidate.name) == wanted_name)
                .map(|candidate| candidate.id);
            if let Some(found) = found {
                map.insert(option.id, found);
                continue;
            }
            if self.target[at].options.len() >= MAX_OPTIONS || !valid_name(&option.name) {
                continue;
            }
            let id = OptionId::generate(self.rng);
            let group = if target_is_status {
                Some(option.group.unwrap_or(StatusGroup::Todo))
            } else {
                None
            };
            self.ops.push(SchemaOp::AddOption {
                property: target_id,
                id,
                name: option.name.clone(),
                color: option.color,
                group,
            });
            self.target[at].options.push(PropertyOption {
                id,
                name: option.name.clone(),
                color: option.color,
                group,
            });
            map.insert(option.id, id);
        }
        self.options.insert(def.id, map);
    }
}

// ----- values -------------------------------------------------------------------------------------

/// A stored value read as the kind of its property: the raw JSON is the `value` of the value's
/// tagged form.
pub(crate) fn decode(kind: PropertyKind, raw: &Value) -> Option<PropertyValue> {
    let mut tagged = Map::new();
    tagged.insert("kind".into(), Value::String(kind.as_str().to_owned()));
    tagged.insert("value".into(), raw.clone());
    serde_json::from_value(Value::Object(tagged)).ok()
}

/// The raw stored form of a value.
pub(crate) fn encode(value: &PropertyValue) -> Option<Value> {
    match serde_json::to_value(value).ok()? {
        Value::Object(mut tagged) => tagged.remove("value"),
        _ => None,
    }
}

/// The option ids a value lists.
fn options_of(value: &PropertyValue) -> Option<Vec<OptionId>> {
    match value {
        PropertyValue::Select(id) | PropertyValue::Status(id) => Some(vec![*id]),
        PropertyValue::MultiSelect(ids) => Some(ids.clone()),
        _ => None,
    }
}

/// What the values of the moved pages came to.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ValueOutcome {
    /// Stored values by target property id, raw JSON, for `StructureDoc::move_in`.
    pub(crate) props: Vec<(PropertyId, Value)>,
    /// Values that could not follow.
    pub(crate) dropped: u32,
}

/// Keeps, in the Person values of a moved page (already mapped to the target schema), the accounts
/// that are members of the target space (ADR-0049: a value lists members when it is written; moving
/// into a personal space keeps this account only). A value left without a person is dropped; the
/// number of values that were dropped is the second answer.
pub(crate) fn keep_members(
    props: Vec<(PropertyId, Value)>,
    target: &[PropertyDef],
    members: &[AccountId],
) -> (Vec<(PropertyId, Value)>, u32) {
    let mut kept = Vec::with_capacity(props.len());
    let mut dropped = 0;
    for (id, raw) in props {
        let is_person = target
            .iter()
            .any(|def| def.id == id && def.kind == PropertyKind::Person);
        if !is_person {
            kept.push((id, raw));
            continue;
        }
        match decode(PropertyKind::Person, &raw) {
            Some(PropertyValue::Person(accounts)) => {
                let members_only: Vec<AccountId> = accounts
                    .into_iter()
                    .filter(|account| members.contains(account))
                    .collect();
                if members_only.is_empty() {
                    dropped += 1;
                } else if let Some(raw) = encode(&PropertyValue::Person(members_only)) {
                    kept.push((id, raw));
                } else {
                    dropped += 1;
                }
            }
            _ => dropped += 1,
        }
    }
    (kept, dropped)
}

/// Maps the stored values of one moved page. `moved` is the set of pages that move along:
/// a relation keeps only those.
pub(crate) fn map_values(
    mapping: &SchemaMapping,
    src: &[PropertyDef],
    moved: &HashSet<PageId>,
    raw: &[(PropertyId, Value)],
) -> ValueOutcome {
    let mut out = ValueOutcome::default();
    for (property, stored) in raw {
        let Some(def) = src.iter().find(|def| def.id == *property) else {
            continue;
        };
        if def.kind.is_computed() {
            continue;
        }
        let Some(value) = decode(def.kind, stored) else {
            continue;
        };
        // A value naming only options the property no longer lists reads as nothing.
        if let Some(options) = options_of(&value)
            && !options
                .iter()
                .any(|o| def.options.iter().any(|d| d.id == *o))
        {
            continue;
        }
        let Some(target_id) = mapping.props.get(property) else {
            out.dropped += 1;
            continue;
        };
        let Some(target) = mapping
            .target
            .iter()
            .find(|candidate| candidate.id == *target_id)
        else {
            out.dropped += 1;
            continue;
        };
        let converted = convert(mapping, def, target, &value, moved);
        match converted {
            Converted::Nothing => {}
            Converted::Lost => out.dropped += 1,
            Converted::Value(value) => match kb_model::normalise_value(&value) {
                Ok(Some(value)) => match encode(&value) {
                    Some(raw) => out.props.push((target.id, raw)),
                    None => out.dropped += 1,
                },
                Ok(None) => {}
                Err(_) => out.dropped += 1,
            },
        }
    }
    out.props.sort_by_key(|(id, _)| *id);
    out
}

enum Converted {
    Value(PropertyValue),
    /// Nothing is left of the value (it was empty, or only held pages that stay).
    Nothing,
    /// The value cannot follow.
    Lost,
}

fn convert(
    mapping: &SchemaMapping,
    src: &PropertyDef,
    dst: &PropertyDef,
    value: &PropertyValue,
    moved: &HashSet<PageId>,
) -> Converted {
    use PropertyValue as V;
    if let V::Relation(pages) = value {
        if dst.kind != PropertyKind::Relation {
            return Converted::Lost;
        }
        let kept: Vec<PageId> = pages
            .iter()
            .copied()
            .filter(|page| moved.contains(page))
            .collect();
        return if kept.is_empty() {
            Converted::Nothing
        } else {
            Converted::Value(V::Relation(kept))
        };
    }
    if let Some(ids) = options_of(value)
        && src.kind.has_options()
        && dst.kind.has_options()
    {
        return select_like(mapping, src, dst, &ids);
    }
    if src.kind == dst.kind {
        return Converted::Value(value.clone());
    }
    let Some(text) = text_of(value, src) else {
        return Converted::Lost;
    };
    match coerce(&text, dst) {
        Some(coerced) => Converted::Value(coerced),
        None => Converted::Lost,
    }
}

/// An option value between two properties that pick options by name.
fn select_like(
    mapping: &SchemaMapping,
    src: &PropertyDef,
    dst: &PropertyDef,
    ids: &[OptionId],
) -> Converted {
    let map = mapping.options.get(&src.id);
    let listed: Vec<OptionId> = ids
        .iter()
        .copied()
        .filter(|id| src.options.iter().any(|option| option.id == *id))
        .collect();
    let mut mapped: Vec<OptionId> = Vec::new();
    for id in &listed {
        if let Some(target) = map.and_then(|map| map.get(id))
            && !mapped.contains(target)
        {
            mapped.push(*target);
        }
    }
    if mapped.is_empty() {
        return Converted::Lost;
    }
    let value = match dst.kind {
        PropertyKind::Select => {
            let first = mapped.first().copied();
            first.map(PropertyValue::Select)
        }
        PropertyKind::Status => mapped.first().copied().map(PropertyValue::Status),
        _ => {
            mapped.truncate(MAX_SELECTED_OPTIONS);
            Some(PropertyValue::MultiSelect(mapped))
        }
    };
    match value {
        Some(value) => Converted::Value(value),
        None => Converted::Lost,
    }
}

/// The text a value reads as, for a property of another kind (`None`: it has none).
fn text_of(value: &PropertyValue, src: &PropertyDef) -> Option<String> {
    use PropertyValue as V;
    match value {
        V::Text(text) | V::Url(text) | V::Email(text) | V::Phone(text) => Some(text.clone()),
        V::Number(number) => Some(format_number(*number)),
        V::Checkbox(on) => Some(if *on { "true" } else { "false" }.to_owned()),
        V::Date(date) => Some(match &date.time {
            Some(time) => format!("{} {time}", date.date),
            None => date.date.clone(),
        }),
        V::Place(place) => Some(place.name.clone()),
        V::Select(id) | V::Status(id) => src
            .options
            .iter()
            .find(|option| option.id == *id)
            .map(|option| option.name.clone()),
        V::MultiSelect(ids) => {
            let names: Vec<&str> = ids
                .iter()
                .filter_map(|id| {
                    src.options
                        .iter()
                        .find(|option| option.id == *id)
                        .map(|option| option.name.as_str())
                })
                .collect();
            (!names.is_empty()).then(|| names.join(", "))
        }
        V::Checklist(items) => {
            let texts: Vec<&str> = items
                .iter()
                .map(|item| item.text.as_str())
                .filter(|text| !text.is_empty())
                .collect();
            (!texts.is_empty()).then(|| texts.join(", "))
        }
        // People are accounts, not words: a Person value only moves into another Person property
        // (`keep_members` then keeps the accounts that are members of the target space), and its
        // names never become text.
        V::Person(_)
        | V::Files(_)
        | V::Relation(_)
        | V::Rollup(_)
        | V::Formula(_)
        | V::CreatedTime(_)
        | V::LastEditedTime(_)
        | V::CreatedBy(_)
        | V::LastEditedBy(_) => None,
    }
}

fn format_number(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{number:.0}")
    } else {
        number.to_string()
    }
}

/// A text as a value of the target property's kind, when it reads as one (the strict rules of
/// the importers: a text that is not clearly a number, a checkbox word or a date stays text).
fn coerce(text: &str, dst: &PropertyDef) -> Option<PropertyValue> {
    use PropertyValue as V;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    match dst.kind {
        PropertyKind::Text => {
            let cleaned: String = text
                .chars()
                .map(|c| {
                    if c.is_control() && !matches!(c, '\t' | '\n' | '\r') {
                        ' '
                    } else {
                        c
                    }
                })
                .collect();
            (cleaned.chars().count() <= MAX_TEXT_CHARS).then_some(V::Text(cleaned))
        }
        PropertyKind::Number => kb_import::values::parse_number(trimmed).map(V::Number),
        PropertyKind::Checkbox => kb_import::values::parse_bool(trimmed).map(V::Checkbox),
        PropertyKind::Date => kb_import::values::parse_date(trimmed)
            .map(|(date, time)| V::Date(DateValue { date, time })),
        PropertyKind::Url => kb_model::valid_url(trimmed).then(|| V::Url(trimmed.to_owned())),
        PropertyKind::Email => one_line(trimmed).map(V::Email),
        PropertyKind::Phone => one_line(trimmed).map(V::Phone),
        PropertyKind::Place => one_line(trimmed).map(|name| {
            V::Place(PlaceValue {
                name,
                lat: None,
                lon: None,
            })
        }),
        PropertyKind::Select | PropertyKind::Status => {
            // An existing option with that name; nothing is added for a text.
            let name = fold(trimmed);
            let option = dst
                .options
                .iter()
                .find(|option| fold(&option.name) == name)?;
            Some(if dst.kind == PropertyKind::Select {
                V::Select(option.id)
            } else {
                V::Status(option.id)
            })
        }
        PropertyKind::MultiSelect => {
            let mut found: Vec<OptionId> = Vec::new();
            for part in trimmed.split(',') {
                let name = fold(part);
                if name.is_empty() {
                    continue;
                }
                let option = dst
                    .options
                    .iter()
                    .find(|option| fold(&option.name) == name)?;
                if !found.contains(&option.id) {
                    found.push(option.id);
                }
            }
            (!found.is_empty()).then_some(V::MultiSelect(found))
        }
        // A text never becomes a list of people.
        PropertyKind::Person
        | PropertyKind::Files
        | PropertyKind::Checklist
        | PropertyKind::Relation
        | PropertyKind::Rollup
        | PropertyKind::CreatedTime
        | PropertyKind::LastEditedTime
        | PropertyKind::CreatedBy
        | PropertyKind::LastEditedBy
        | PropertyKind::Formula => None,
    }
}

/// The text when it is one line without control characters.
fn one_line(text: &str) -> Option<String> {
    (!text.is_empty() && !text.chars().any(char::is_control)).then(|| text.to_owned())
}

// ----- views --------------------------------------------------------------------------------------

fn property_id_of(value: &Value) -> Option<PropertyId> {
    value.as_str()?.parse().ok()
}

fn option_id_of(value: &Value) -> Option<OptionId> {
    value.as_str()?.parse().ok()
}

/// Whether a filter operator takes option ids as its value: `hasOption` and, since ADR-0058,
/// `lacksOption`. (`relatesTo` and `notRelatesTo` take page ids, which a move keeps.)
fn takes_option_ids(op: Option<&Value>) -> bool {
    matches!(
        op.and_then(Value::as_str),
        Some("hasOption" | "lacksOption")
    )
}

/// The option ids in a filter value: one id or a list of them.
fn option_ids_of(value: Option<&Value>) -> Vec<OptionId> {
    match value {
        Some(Value::Array(items)) => items.iter().filter_map(option_id_of).collect(),
        Some(single) => option_id_of(single).into_iter().collect(),
        None => Vec::new(),
    }
}

/// The deepest nesting of filter groups followed (reading takes one level; a stored tree is only as
/// deep as a hostile document makes it, and what is deeper is left as stored).
const MAX_GROUP_DEPTH: usize = 8;

/// Whether a stored filter tree node is a group (it has an `items` list) and not a filter.
fn is_group(node: &Value) -> bool {
    node.get("items").is_some_and(Value::is_array)
}

/// The target property of a stored property id, when it has one.
fn mapped_property(mapping: &SchemaMapping, id: &Value) -> Option<String> {
    let source = property_id_of(id)?;
    mapping.props.get(&source).map(ToString::to_string)
}

/// The target option of a stored option id of the source property `property`, when it has one.
fn mapped_option(mapping: &SchemaMapping, property: PropertyId, id: &Value) -> Option<String> {
    let source = option_id_of(id)?;
    mapping
        .options
        .get(&property)
        .and_then(|map| map.get(&source))
        .map(ToString::to_string)
}

/// One stored filter with its property and, for a `hasOption` or `lacksOption`, its option ids remapped.
/// `None`: it has no counterpart in the target (the property or every option is gone). A filter that names
/// no property id, or an entry that is no object, is kept as stored; so is every key this version does not
/// know, and the value of every other operator (the page ids of `relatesTo` and `notRelatesTo`, a day, a
/// relative value such as `{ "relative": "today" }`).
fn remap_filter(filter: &Value, mapping: &SchemaMapping) -> Option<Value> {
    let Value::Object(object) = filter else {
        return Some(filter.clone());
    };
    let Some(source) = object.get("propertyId").and_then(property_id_of) else {
        return Some(filter.clone());
    };
    let mapped = mapping.props.get(&source)?.to_string();
    let mut next = object.clone();
    next.insert("propertyId".into(), Value::String(mapped));
    if takes_option_ids(object.get("op")) {
        match object.get("value") {
            Some(Value::Array(ids)) => {
                let options: Vec<Value> = ids
                    .iter()
                    .filter_map(|id| mapped_option(mapping, source, id).map(Value::String))
                    .collect();
                if options.is_empty() {
                    return None;
                }
                next.insert("value".into(), Value::Array(options));
            }
            Some(single) => {
                let option = mapped_option(mapping, source, single)?;
                next.insert("value".into(), Value::String(option));
            }
            None => {}
        }
    }
    Some(Value::Object(next))
}

/// A stored filter group with its filters remapped (nested groups too). A filter with no counterpart goes;
/// a group that is left with no item after it had some goes with them (`None`), one that was empty stays.
/// What is no group is kept as stored.
fn remap_group(group: &Value, mapping: &SchemaMapping, depth: usize) -> Option<Value> {
    let (Value::Object(object), Some(Value::Array(items))) = (group, group.get("items")) else {
        return Some(group.clone());
    };
    if depth > MAX_GROUP_DEPTH {
        return Some(group.clone());
    }
    let kept: Vec<Value> = items
        .iter()
        .filter_map(|item| {
            if is_group(item) {
                remap_group(item, mapping, depth + 1)
            } else {
                remap_filter(item, mapping)
            }
        })
        .collect();
    if kept.is_empty() && !items.is_empty() {
        return None;
    }
    let mut next = object.clone();
    next.insert("items".into(), Value::Array(kept));
    Some(Value::Object(next))
}

/// One sort key (`sort`, or an entry of `sorts`): its `by` as the target has it (`title`, `updated` and
/// `created` stay), `None` when its property has no counterpart. An entry that is no object, or names no
/// `by`, is kept as stored; so is every other key.
fn remap_sort_key(key: &Value, mapping: &SchemaMapping) -> Option<Value> {
    let Value::Object(object) = key else {
        return Some(key.clone());
    };
    match object.get("by") {
        Some(Value::String(by)) if matches!(by.as_str(), "title" | "updated" | "created") => {
            Some(key.clone())
        }
        Some(by) => mapped_property(mapping, by).map(|mapped| {
            let mut next = object.clone();
            next.insert("by".into(), Value::String(mapped));
            Value::Object(next)
        }),
        None => Some(key.clone()),
    }
}

/// A calendar date property (`dateBy`, `endBy`) as the target has it: the id of its counterpart, `None`
/// when it has none (the calendar then asks to choose). `created`, `updated` and whatever is no property
/// id stay as stored.
fn remap_date_property(value: &Value, mapping: &SchemaMapping) -> Option<Value> {
    match value
        .as_str()
        .and_then(|text| text.parse::<PropertyId>().ok())
    {
        Some(source) => mapping
            .props
            .get(&source)
            .map(|mapped| Value::String(mapped.to_string())),
        None => Some(value.clone()),
    }
}

/// The attributes of a `pageList` block that change when its ids are remapped to the target
/// schema: `Some(value)` sets the attribute, `None` removes it (an id with no counterpart goes;
/// a view keeps everything else). The block's own attributes and every view of `views` (ADR-0054) go
/// through the same code, so the mirror of the first view and the view stay equal; a `views` entry that
/// is no object, and everything in a view that this version does not understand, stays as stored.
pub(crate) fn remap_view(
    attrs: &Map<String, Value>,
    mapping: &SchemaMapping,
) -> Vec<(&'static str, Option<Value>)> {
    let mut changes = remap_parts(attrs, mapping, true);
    if let Some(Value::Array(views)) = attrs.get(VIEWS_ATTR) {
        let after = Value::Array(
            views
                .iter()
                .map(|view| remap_one_view(view, mapping))
                .collect(),
        );
        if attrs.get(VIEWS_ATTR) != Some(&after) {
            changes.push((VIEWS_ATTR, Some(after)));
        }
    }
    changes
}

/// One view of `views` with its ids remapped.
fn remap_one_view(view: &Value, mapping: &SchemaMapping) -> Value {
    let Value::Object(object) = view else {
        return view.clone();
    };
    let mut next = object.clone();
    for (key, value) in remap_parts(object, mapping, false) {
        match value {
            Some(value) => {
                next.insert(key.to_owned(), value);
            }
            None => {
                next.remove(key);
            }
        }
    }
    Value::Object(next)
}

/// The changes to the attributes of a block (`is_block`) or of one of its views: a view has no `source`
/// of its own, and a tree `filter`, `dateBy` and `endBy` beside the legacy attributes.
fn remap_parts(
    attrs: &Map<String, Value>,
    mapping: &SchemaMapping,
    is_block: bool,
) -> Vec<(&'static str, Option<Value>)> {
    let mut changes: Vec<(&'static str, Option<Value>)> = Vec::new();
    let property = |id: &Value| mapped_property(mapping, id);
    let option = |property: PropertyId, id: &Value| mapped_option(mapping, property, id);
    let mut set = |key: &'static str, before: Option<&Value>, after: Option<Value>| {
        if before != after.as_ref() {
            changes.push((key, after));
        }
    };

    if let Some(Value::Array(ids)) = attrs.get("properties") {
        let mapped: Vec<Value> = ids
            .iter()
            .filter_map(|id| property(id).map(Value::String))
            .collect();
        set(
            "properties",
            attrs.get("properties"),
            Some(Value::Array(mapped)),
        );
    }
    // The sort keys of a view (ADR-0058): each key's property follows like `sort.by`; a key with no
    // counterpart goes, and `sort`, which mirrors the first key, then follows the first key left (both go
    // when none is left).
    let mut sort_follows: Option<Option<Value>> = None;
    if let Some(Value::Array(keys)) = attrs.get("sorts") {
        let mapped: Vec<Value> = keys
            .iter()
            .filter_map(|key| remap_sort_key(key, mapping))
            .collect();
        if mapped.len() < keys.len() {
            match mapped.first() {
                Some(first) if first.is_object() => sort_follows = Some(Some(first.clone())),
                Some(_) => {}
                None => sort_follows = Some(None),
            }
        }
        let after = (!mapped.is_empty() || keys.is_empty()).then_some(Value::Array(mapped));
        set("sorts", attrs.get("sorts"), after);
    }
    if let Some(follow) = sort_follows.filter(|_| attrs.contains_key("sort")) {
        set("sort", attrs.get("sort"), follow);
    } else if let Some(sort @ Value::Object(_)) = attrs.get("sort") {
        set("sort", Some(sort), remap_sort_key(sort, mapping));
    }
    if let Some(Value::Array(filters)) = attrs.get("filters") {
        let kept: Vec<Value> = filters
            .iter()
            .filter_map(|filter| remap_filter(filter, mapping))
            .collect();
        set("filters", attrs.get("filters"), Some(Value::Array(kept)));
    }
    // The filter tree of a view; one that is emptied by what has no counterpart goes (an absent one is
    // the empty AND group, which says nothing).
    if let Some(tree) = attrs.get("filter")
        && is_group(tree)
    {
        set("filter", Some(tree), remap_group(tree, mapping, 0));
    }
    // The calendar's date properties; one with no counterpart goes, and the calendar asks to choose.
    for key in ["dateBy", "endBy"] {
        if let Some(stored) = attrs.get(key) {
            set(key, Some(stored), remap_date_property(stored, mapping));
        }
    }
    let group = attrs.get("groupBy").and_then(property_id_of);
    if let Some(source) = group {
        match attrs.get("groupBy").and_then(property) {
            Some(mapped) => {
                set("groupBy", attrs.get("groupBy"), Some(Value::String(mapped)));
                if let Some(Value::Array(hidden)) = attrs.get("hiddenGroups") {
                    let mapped_hidden: Vec<Value> = hidden
                        .iter()
                        .filter_map(|entry| {
                            if entry.as_str() == Some("none") {
                                Some(entry.clone())
                            } else {
                                option(source, entry).map(Value::String)
                            }
                        })
                        .collect();
                    set(
                        "hiddenGroups",
                        attrs.get("hiddenGroups"),
                        Some(Value::Array(mapped_hidden)),
                    );
                }
            }
            None => {
                set("groupBy", attrs.get("groupBy"), None);
                if attrs.contains_key("hiddenGroups") {
                    set("hiddenGroups", attrs.get("hiddenGroups"), None);
                }
            }
        }
    }
    if let Some(Value::Object(widths)) = attrs.get("columnWidths") {
        let mut mapped = Map::new();
        for (key, width) in widths {
            if key == "title" {
                mapped.insert(key.clone(), width.clone());
            } else if let Some(target) = property(&Value::String(key.clone())) {
                mapped.insert(target, width.clone());
            }
        }
        set(
            "columnWidths",
            attrs.get("columnWidths"),
            Some(Value::Object(mapped)),
        );
    }
    // The footer's functions follow their columns; with no column left the attribute goes (absent
    // is "none"), and what is kept is as stored, a function of a newer version included.
    if let Some(Value::Object(calculations)) = attrs.get("calculations") {
        let mut mapped = Map::new();
        for (key, function) in calculations {
            if key == "title" {
                mapped.insert(key.clone(), function.clone());
            } else if let Some(target) = property(&Value::String(key.clone())) {
                mapped.insert(target, function.clone());
            }
        }
        set(
            "calculations",
            attrs.get("calculations"),
            (!mapped.is_empty()).then_some(Value::Object(mapped)),
        );
    }
    // The widgets of a dashboard (ADR-0055): each widget's ids follow like a view's.
    if let Some(Value::Array(widgets)) = attrs.get("widgets") {
        let mapped: Vec<Value> = widgets
            .iter()
            .map(|widget| remap_widget(widget, mapping))
            .collect();
        set("widgets", attrs.get("widgets"), Some(Value::Array(mapped)));
    }
    if is_block
        && let Some(source) = attrs.get("source")
        && source.get("kind").and_then(Value::as_str) == Some("tag")
    {
        let after = source
            .get("optionId")
            .and_then(|id| option(TAGS_PROPERTY_ID, id))
            .map(|mapped| {
                let mut next = source.as_object().cloned().unwrap_or_default();
                next.insert("optionId".into(), Value::String(mapped));
                Value::Object(next)
            });
        set("source", attrs.get("source"), after);
    }
    changes
}

/// One widget of a dashboard with its property ids remapped (`dashboards.md` section 5): its own
/// filter (a filter tree like a view's; one emptied by what has no counterpart goes), the column of a
/// number (`title` stays; with no counterpart the number counts all pages again), the grouping of a
/// chart (`created` and `updated` stay; with no counterpart it asks to choose: `null`) and the measure
/// of a chart (with no counterpart it counts). `id`, `kind`, `title`, `w`, `h`, `viewId` and every key
/// this version does not know are untouched; an entry that is no object is kept as stored.
fn remap_widget(widget: &Value, mapping: &SchemaMapping) -> Value {
    let Value::Object(object) = widget else {
        return widget.clone();
    };
    let mut next = object.clone();
    if let Some(filter) = object.get("filter")
        && is_group(filter)
    {
        match remap_group(filter, mapping, 0) {
            Some(mapped) => {
                next.insert("filter".into(), mapped);
            }
            None => {
                next.remove("filter");
            }
        }
    }
    if let Some(Value::Object(calc)) = object.get("calc")
        && let Some(by) = calc.get("by")
        && property_id_of(by).is_some()
    {
        let mut after = calc.clone();
        match mapped_property(mapping, by) {
            Some(mapped) => {
                after.insert("by".into(), Value::String(mapped));
            }
            None => {
                after.insert("by".into(), Value::String("title".into()));
                after.insert("fn".into(), Value::String("count_all".into()));
            }
        }
        next.insert("calc".into(), Value::Object(after));
    }
    if let Some(Value::Object(chart)) = object.get("chart") {
        let mut after = chart.clone();
        if let Some(group) = chart.get("groupBy")
            && property_id_of(group).is_some()
        {
            after.insert(
                "groupBy".into(),
                mapped_property(mapping, group).map_or(Value::Null, Value::String),
            );
        }
        if let Some(Value::Object(measure)) = chart.get("measure")
            && let Some(property) = measure.get("propertyId")
        {
            match mapped_property(mapping, property) {
                Some(mapped) => {
                    let mut kept = measure.clone();
                    kept.insert("propertyId".into(), Value::String(mapped));
                    after.insert("measure".into(), Value::Object(kept));
                }
                None => {
                    let mut counting = Map::new();
                    counting.insert("fn".into(), Value::String("count".into()));
                    after.insert("measure".into(), Value::Object(counting));
                }
            }
        }
        next.insert("chart".into(), Value::Object(after));
    }
    Value::Object(next)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use kb_types::testing::SeededRng;
    use serde_json::json;

    use super::*;

    fn def(id: u8, name: &str, kind: PropertyKind) -> PropertyDef {
        PropertyDef {
            id: PropertyId::from_bytes([id; 16]),
            name: name.to_owned(),
            kind,
            options: Vec::new(),
            created_at: 0,
            config: match kind {
                PropertyKind::Relation => Some(PropertyConfig::Relation {
                    two_way: false,
                    paired_property_id: None,
                }),
                _ => None,
            },
        }
    }

    fn option(id: u8, name: &str) -> PropertyOption {
        PropertyOption {
            id: OptionId::from_bytes([id; 16]),
            name: name.to_owned(),
            color: OptionColor::Blue,
            group: None,
        }
    }

    fn tags() -> PropertyDef {
        PropertyDef {
            id: TAGS_PROPERTY_ID,
            options: Vec::new(),
            ..def(0, "Tags", PropertyKind::MultiSelect)
        }
    }

    fn needs(src: &[PropertyDef], pages: &[Vec<(PropertyId, Value)>]) -> Needs {
        let mut needs = Needs::default();
        for raw in pages {
            needs.add_values(src, raw);
        }
        needs
    }

    #[test]
    fn a_placeholder_is_a_kind_of_value_it_needs_its_property_and_follows_the_mapping() {
        let rng = SeededRng::new(1);
        let (due, owner, notes) = (
            def(1, "Due", PropertyKind::Date),
            def(2, "Owner", PropertyKind::Person),
            def(3, "Notes", PropertyKind::Text),
        );
        let src = vec![tags(), due.clone(), owner.clone(), notes.clone()];
        // The target has an Owner of its own (another id) and no Due.
        let dst = vec![tags(), def(9, "owner", PropertyKind::Person)];
        let entries = [
            (due.id, TemplatePlaceholder::Today),
            (owner.id, TemplatePlaceholder::Me),
            // A placeholder the source property does not take, and one on an unknown property.
            (notes.id, TemplatePlaceholder::Today),
            (PropertyId::from_bytes([77; 16]), TemplatePlaceholder::Now),
        ];
        let mut wanted = Needs::default();
        wanted.add_placeholders(&src, &entries);
        assert_eq!(
            wanted.props,
            BTreeSet::from([due.id, owner.id]),
            "only the properties that take their placeholder need a counterpart"
        );
        let mapping = plan_schema(&src, &dst, &wanted, &rng);
        assert_eq!(mapping.created, 1, "Due is made, Owner is matched by name");
        let mapped = map_placeholders(&mapping, &entries);
        let new_due = mapping.props[&due.id];
        let mut expected = vec![
            (new_due, TemplatePlaceholder::Today),
            (PropertyId::from_bytes([9; 16]), TemplatePlaceholder::Me),
        ];
        expected.sort_unstable_by_key(|(id, _)| *id);
        assert_eq!(mapped, expected);

        // Without a counterpart a placeholder is dropped: the target has no room for Due.
        let mut full = vec![tags()];
        for n in 0..MAX_PROPERTIES - 1 {
            full.push(def(
                u8::try_from(100 + n).unwrap(),
                &format!("Filler {n}"),
                PropertyKind::Text,
            ));
        }
        assert_eq!(full.len(), MAX_PROPERTIES);
        let mapping = plan_schema(&src, &full, &wanted, &rng);
        assert!(!mapping.props.contains_key(&due.id));
        let mapped = map_placeholders(&mapping, &entries);
        assert!(mapped.is_empty(), "no room for Due or Owner: {mapped:?}");

        // A counterpart whose kind does not take the placeholder is not given it either.
        let mut by_hand = SchemaMapping::default();
        by_hand
            .props
            .insert(due.id, PropertyId::from_bytes([5; 16]));
        by_hand.target = vec![def(5, "Due", PropertyKind::Text)];
        assert!(map_placeholders(&by_hand, &[(due.id, TemplatePlaceholder::Now)]).is_empty());
        // Two source properties that map to one target property keep the first placeholder.
        let mut twice = SchemaMapping::default();
        twice.props.insert(due.id, PropertyId::from_bytes([6; 16]));
        twice.props.insert(
            PropertyId::from_bytes([8; 16]),
            PropertyId::from_bytes([6; 16]),
        );
        twice.target = vec![def(6, "Due", PropertyKind::Date)];
        let mapped = map_placeholders(
            &twice,
            &[
                (due.id, TemplatePlaceholder::Today),
                (PropertyId::from_bytes([8; 16]), TemplatePlaceholder::Now),
            ],
        );
        assert_eq!(mapped.len(), 1);
    }

    #[test]
    fn a_property_of_the_same_name_and_kind_is_reused_and_a_missing_one_is_created() {
        let rng = SeededRng::new(1);
        let mut status = def(2, "Priority", PropertyKind::Select);
        status.options = vec![option(21, "High"), option(22, "Low")];
        let src = vec![tags(), def(1, "Due", PropertyKind::Date), status];
        let mut target_priority = def(9, "priority", PropertyKind::Select);
        target_priority.options = vec![option(91, "high")];
        let dst = vec![tags(), target_priority];
        let raw = vec![vec![
            (src[1].id, json!({"date": "2026-10-13"})),
            (src[2].id, json!(src[2].options[0].id.to_string())),
        ]];
        let mapping = plan_schema(&src, &dst, &needs(&src, &raw), &rng);

        assert_eq!(mapping.created, 1, "only Due is new");
        assert_eq!(
            mapping.props.get(&src[2].id),
            Some(&PropertyId::from_bytes([9; 16])),
            "Priority matches by folded name"
        );
        let map = &mapping.options[&src[2].id];
        assert_eq!(
            map.get(&OptionId::from_bytes([21; 16])),
            Some(&OptionId::from_bytes([91; 16])),
            "the option maps by name"
        );
        assert!(
            !map.contains_key(&OptionId::from_bytes([22; 16])),
            "an option nothing refers to is not added to a property that exists"
        );
        let due = mapping.props[&src[1].id];
        let created = mapping.target.iter().find(|d| d.id == due).unwrap();
        assert_eq!(
            (created.name.as_str(), created.kind),
            ("Due", PropertyKind::Date)
        );

        let outcome = map_values(&mapping, &src, &HashSet::new(), &raw[0]);
        assert_eq!(outcome.dropped, 0);
        assert_eq!(outcome.props.len(), 2);
    }

    #[test]
    fn a_full_target_schema_drops_the_values_that_cannot_follow() {
        let rng = SeededRng::new(2);
        let src = vec![tags(), def(1, "Due", PropertyKind::Date)];
        let mut dst = vec![tags()];
        for n in 0..(MAX_PROPERTIES - 1) {
            dst.push(def(
                u8::try_from(n + 10).unwrap(),
                &format!("P{n}"),
                PropertyKind::Text,
            ));
        }
        let raw = vec![(src[1].id, json!({"date": "2026-10-13"}))];
        let mapping = plan_schema(&src, &dst, &needs(&src, std::slice::from_ref(&raw)), &rng);
        assert_eq!(mapping.created, 0);
        let outcome = map_values(&mapping, &src, &HashSet::new(), &raw);
        assert_eq!(outcome.dropped, 1);
        assert!(outcome.props.is_empty());
    }

    #[test]
    fn a_value_of_another_kind_follows_when_its_text_reads_as_the_kind() {
        let rng = SeededRng::new(3);
        let src = vec![
            tags(),
            def(1, "Score", PropertyKind::Text),
            def(2, "Done", PropertyKind::Text),
            def(3, "Link", PropertyKind::Number),
        ];
        let dst = vec![
            tags(),
            def(11, "Score", PropertyKind::Number),
            def(12, "Done", PropertyKind::Checkbox),
            def(13, "Link", PropertyKind::Text),
        ];
        let raw = vec![
            (src[1].id, json!("12.5")),
            (src[2].id, json!("maybe")),
            (src[3].id, json!(3.0)),
        ];
        let mapping = plan_schema(&src, &dst, &needs(&src, std::slice::from_ref(&raw)), &rng);
        assert_eq!(mapping.created, 0, "same names bind whatever the kind");
        let outcome = map_values(&mapping, &src, &HashSet::new(), &raw);
        assert_eq!(outcome.dropped, 1, "\"maybe\" is no checkbox word");
        let by_target: HashMap<PropertyId, Value> = outcome.props.into_iter().collect();
        assert_eq!(by_target[&dst[1].id], json!(12.5));
        assert_eq!(by_target[&dst[3].id], json!("3"));
        assert!(!by_target.contains_key(&dst[2].id));
    }

    #[test]
    fn a_status_property_keeps_its_options_and_the_unused_defaults_go() {
        let rng = SeededRng::new(4);
        let mut stage = def(1, "Stage", PropertyKind::Status);
        stage.options = vec![
            PropertyOption {
                group: Some(StatusGroup::Todo),
                ..option(11, "Not started")
            },
            PropertyOption {
                group: Some(StatusGroup::Done),
                ..option(12, "Shipped")
            },
        ];
        let src = vec![tags(), stage];
        let raw = vec![(src[1].id, json!(src[1].options[1].id.to_string()))];
        let mapping = plan_schema(
            &src,
            &[tags()],
            &needs(&src, std::slice::from_ref(&raw)),
            &rng,
        );
        let created = &mapping.target[1];
        let names: Vec<&str> = created.options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            names,
            ["Not started", "Shipped"],
            "In progress and Done were dropped"
        );
        assert!(
            mapping
                .ops
                .iter()
                .filter(|op| matches!(op, SchemaOp::DeleteOption { .. }))
                .count()
                == 2
        );
        let outcome = map_values(&mapping, &src, &HashSet::new(), &raw);
        assert_eq!(outcome.dropped, 0);
        assert_eq!(
            outcome.props[0].1,
            json!(created.options[1].id.to_string()),
            "the value names the new option"
        );
    }

    #[test]
    fn a_relation_keeps_the_pages_that_move_and_a_two_way_pair_is_created_as_a_pair() {
        let rng = SeededRng::new(5);
        let mut forward = def(1, "Blocks", PropertyKind::Relation);
        let mut back = def(2, "Blocks (back)", PropertyKind::Relation);
        forward.config = Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(back.id),
        });
        back.config = Some(PropertyConfig::Relation {
            two_way: true,
            paired_property_id: Some(forward.id),
        });
        let src = vec![tags(), forward, back];
        let (a, b, c) = (
            PageId::from_bytes([1; 16]),
            PageId::from_bytes([2; 16]),
            PageId::from_bytes([3; 16]),
        );
        let moved: HashSet<PageId> = [a, b].into_iter().collect();
        let raw_a = vec![
            (src[1].id, json!([b.to_string(), c.to_string()])),
            (src[2].id, json!([c.to_string()])),
        ];
        let mapping = plan_schema(
            &src,
            &[tags()],
            &needs(&src, std::slice::from_ref(&raw_a)),
            &rng,
        );
        assert_eq!(mapping.created, 2);
        assert!(
            mapping
                .ops
                .iter()
                .any(|op| matches!(op, SchemaOp::RelationPair { name, back_name, .. } if name == "Blocks" && back_name == "Blocks (back)"))
        );
        let outcome = map_values(&mapping, &src, &moved, &raw_a);
        assert_eq!(outcome.dropped, 0);
        assert_eq!(
            outcome.props.len(),
            1,
            "the back value held only a page that stays"
        );
        assert_eq!(outcome.props[0].1, json!([b.to_string()]));

        // Only one half used: a one-way relation.
        let raw_one = vec![(src[1].id, json!([b.to_string()]))];
        let mapping = plan_schema(&src, &[tags()], &needs(&src, &[raw_one]), &rng);
        assert_eq!(mapping.created, 1);
        assert!(
            mapping
                .ops
                .iter()
                .all(|op| !matches!(op, SchemaOp::RelationPair { .. }))
        );
    }

    #[test]
    fn a_view_follows_the_mapping_and_loses_what_has_no_counterpart() {
        let rng = SeededRng::new(6);
        let mut priority = def(2, "Priority", PropertyKind::Select);
        priority.options = vec![option(21, "High"), option(22, "Low")];
        let gone = def(3, "Gone", PropertyKind::Text);
        let src = vec![tags(), def(1, "Due", PropertyKind::Date), priority, gone];
        let attrs = json!({
            "id": "b",
            "layout": "board",
            "source": {"kind": "children"},
            "properties": [src[1].id.to_string(), src[2].id.to_string(), src[3].id.to_string()],
            "sort": {"by": src[1].id.to_string(), "dir": "desc"},
            "filters": [
                {"propertyId": src[2].id.to_string(), "op": "hasOption", "value": src[2].options[0].id.to_string()},
                {"propertyId": src[3].id.to_string(), "op": "isSet"},
            ],
            "groupBy": src[2].id.to_string(),
            "hiddenGroups": [src[2].options[1].id.to_string(), "none"],
            "columnWidths": {"title": 200, src[1].id.to_string(): 120, src[3].id.to_string(): 90},
            "calculations": {"title": "count_all", src[1].id.to_string(): "earliest", src[3].id.to_string(): "count_all"},
        });
        let attrs = attrs.as_object().unwrap().clone();
        let mut needs = Needs::default();
        needs.add_view(&attrs);
        // The Gone property has no room in a full target.
        let mut dst = vec![tags()];
        for n in 0..(MAX_PROPERTIES - 3) {
            dst.push(def(
                u8::try_from(n + 10).unwrap(),
                &format!("P{n}"),
                PropertyKind::Text,
            ));
        }
        let mapping = plan_schema(&src, &dst, &needs, &rng);
        assert_eq!(mapping.created, 2, "Due and Priority fit, Gone does not");

        let changes: HashMap<&str, Option<Value>> =
            remap_view(&attrs, &mapping).into_iter().collect();
        let due = mapping.props[&src[1].id].to_string();
        let priority_id = mapping.props[&src[2].id].to_string();
        let high = mapping.options[&src[2].id][&src[2].options[0].id].to_string();
        let low = mapping.options[&src[2].id][&src[2].options[1].id].to_string();
        assert_eq!(changes["properties"], Some(json!([due, priority_id])));
        assert_eq!(changes["sort"], Some(json!({"by": due, "dir": "desc"})));
        assert_eq!(
            changes["filters"],
            Some(json!([{"propertyId": priority_id, "op": "hasOption", "value": high}]))
        );
        assert_eq!(changes["groupBy"], Some(json!(priority_id)));
        assert_eq!(changes["hiddenGroups"], Some(json!([low, "none"])));
        assert_eq!(
            changes["columnWidths"],
            Some(json!({"title": 200, due.clone(): 120}))
        );
        assert_eq!(
            changes["calculations"],
            Some(json!({"title": "count_all", due: "earliest"}))
        );
        assert!(
            !changes.contains_key("source"),
            "a children source is not touched"
        );
        assert!(!changes.contains_key("layout"));
    }

    #[test]
    fn a_view_of_a_property_that_maps_to_itself_changes_nothing() {
        let rng = SeededRng::new(7);
        let src = vec![tags(), def(1, "Due", PropertyKind::Date)];
        let attrs =
            json!({"properties": [src[1].id.to_string()], "sort": {"by": "title", "dir": "asc"}});
        let attrs = attrs.as_object().unwrap().clone();
        let mut needs = Needs::default();
        needs.add_view(&attrs);
        // The target has the same property under the same id (an earlier move back and forth).
        let dst = src.clone();
        let mapping = plan_schema(&src, &dst, &needs, &rng);
        assert!(remap_view(&attrs, &mapping).is_empty());
    }

    // ----- the views of ADR-0054 -------------------------------------------------------------

    fn pid(n: u8) -> String {
        PropertyId::from_bytes([n; 16]).to_string()
    }

    fn oid(n: u8) -> String {
        OptionId::from_bytes([n; 16]).to_string()
    }

    /// A mapping made by hand: property 1 (Due) -> 101, 2 (End) -> 102, 3 (Priority) -> 103 with
    /// its option 31 (High) -> 131; 4 (Gone), the option 32 (Low) and Tags have no counterpart.
    fn mapping() -> SchemaMapping {
        let mut mapping = SchemaMapping::default();
        for (from, to) in [(1, 101), (2, 102), (3, 103)] {
            mapping.props.insert(
                PropertyId::from_bytes([from; 16]),
                PropertyId::from_bytes([to; 16]),
            );
        }
        mapping.options.insert(
            PropertyId::from_bytes([3; 16]),
            HashMap::from([(
                OptionId::from_bytes([31; 16]),
                OptionId::from_bytes([131; 16]),
            )]),
        );
        mapping
    }

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn has_option(property: u8, option: u8) -> Value {
        json!({ "propertyId": pid(property), "op": "hasOption", "value": oid(option) })
    }

    fn is_set(property: u8) -> Value {
        json!({ "propertyId": pid(property), "op": "isSet" })
    }

    /// The block of the design document: the mirror of its first view, then four views.
    fn block_with_views() -> Map<String, Value> {
        let tree = json!({ "op": "and", "items": [
            has_option(3, 31),
            is_set(4),
            { "op": "or", "items": [has_option(3, 32), is_set(4)] },
            { "op": "or", "items": [is_set(1), is_set(4)] },
            { "op": "or", "items": [] }
        ]});
        object(json!({
            "id": "b",
            "source": { "kind": "children" },
            "layout": "table",
            "properties": [pid(1), pid(3), pid(4)],
            "sort": { "by": pid(1), "dir": "asc" },
            "filters": [has_option(3, 31), is_set(4)],
            "columnWidths": { "title": 320, pid(1): 120, pid(4): 90 },
            "calculations": { "title": "count_all", pid(4): "sum" },
            "views": [
                { "id": "k3f9a2bc", "name": "", "layout": "table",
                  "properties": [pid(1), pid(3), pid(4)],
                  "sort": { "by": pid(1), "dir": "asc" },
                  "filter": tree,
                  "columnWidths": { "title": 320, pid(1): 120, pid(4): 90 },
                  "calculations": { "title": "count_all", pid(4): "sum" } },
                { "id": "b81d00e4", "name": "By priority", "layout": "board", "properties": [pid(1)],
                  "sort": { "by": "title", "dir": "asc" }, "groupBy": pid(3),
                  "hiddenGroups": [oid(31), oid(32), "none"], "openIn": "center",
                  "futureKey": { "ids": [pid(1)] } },
                { "id": "7c0a55d1", "name": "Schedule", "layout": "calendar", "properties": [pid(3)],
                  "sort": { "by": "updated", "dir": "asc" }, "dateBy": pid(1), "endBy": pid(2) },
                { "id": "00000001", "name": "Lost", "layout": "calendar", "properties": [],
                  "sort": { "by": pid(4), "dir": "asc" }, "dateBy": pid(4), "endBy": pid(4),
                  "groupBy": pid(4), "hiddenGroups": [oid(31)],
                  "filter": { "op": "or", "items": [is_set(4), has_option(3, 32)] } },
                { "id": "e2a9b6f0", "name": "", "layout": "gallery", "properties": [],
                  "sort": { "by": "created", "dir": "desc" }, "dateBy": "created", "cardSize": "l",
                  "filter": "not a group" },
                "not a view",
                7
            ]
        }))
    }

    #[test]
    fn every_view_follows_the_mapping_like_the_block_and_the_mirror_stays_equal() {
        let attrs = block_with_views();
        let changes: HashMap<&str, Option<Value>> =
            remap_view(&attrs, &mapping()).into_iter().collect();
        let d = PropertyId::from_bytes([101; 16]).to_string();
        let e = PropertyId::from_bytes([102; 16]).to_string();
        let p = PropertyId::from_bytes([103; 16]).to_string();
        let high = OptionId::from_bytes([131; 16]).to_string();
        let new_high = json!({ "propertyId": p, "op": "hasOption", "value": high });
        let new_due_set = json!({ "propertyId": d, "op": "isSet" });

        // The block's own attributes, as before.
        assert_eq!(changes["properties"], Some(json!([d, p])));
        assert_eq!(changes["sort"], Some(json!({ "by": d, "dir": "asc" })));
        assert_eq!(changes["filters"], Some(json!([new_high])));
        assert_eq!(
            changes["columnWidths"],
            Some(json!({ "title": 320, d.clone(): 120 }))
        );
        assert_eq!(
            changes["calculations"],
            Some(json!({ "title": "count_all" }))
        );

        // Every view.
        let Some(Some(Value::Array(views))) = changes.get("views") else {
            panic!("the views change");
        };
        assert_eq!(views.len(), 7);
        // The first view: what the mirror says, the nested group that lost every filter gone, the one that
        // keeps a filter and the one that was empty from the start stay.
        let first = &views[0];
        assert_eq!(first["properties"], changes["properties"].clone().unwrap());
        assert_eq!(first["sort"], changes["sort"].clone().unwrap());
        assert_eq!(
            first["columnWidths"],
            changes["columnWidths"].clone().unwrap()
        );
        assert_eq!(
            first["calculations"],
            changes["calculations"].clone().unwrap()
        );
        assert_eq!(
            first["filter"],
            json!({ "op": "and", "items": [
                new_high,
                { "op": "or", "items": [new_due_set] },
                { "op": "or", "items": [] }
            ]})
        );
        // The plain filters of the first view's AND group are the mirror's `filters`.
        let plain: Vec<Value> = first["filter"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item.get("items").is_none())
            .cloned()
            .collect();
        assert_eq!(Some(Value::Array(plain)), changes["filters"]);
        assert_eq!(
            (first["id"].as_str(), first["name"].as_str()),
            (Some("k3f9a2bc"), Some(""))
        );

        // The board: the group, the hidden columns (the option with no counterpart goes, `none` stays), and
        // what this version does not know and `openIn` untouched.
        assert_eq!(views[1]["groupBy"], json!(p));
        assert_eq!(views[1]["hiddenGroups"], json!([high, "none"]));
        assert_eq!(views[1]["properties"], json!([d]));
        assert_eq!(views[1]["openIn"], json!("center"));
        assert_eq!(views[1]["sort"], json!({ "by": "title", "dir": "asc" }));
        assert_eq!(
            views[1]["futureKey"],
            json!({ "ids": [pid(1)] }),
            "extra keys are as stored"
        );

        // The calendar: both date properties.
        assert_eq!(views[2]["dateBy"], json!(d));
        assert_eq!(views[2]["endBy"], json!(e));
        assert_eq!(views[2]["properties"], json!([p]));
        assert_eq!(views[2]["sort"], json!({ "by": "updated", "dir": "asc" }));

        // A calendar whose properties have no counterpart: they go, the calendar then asks to choose; the
        // filter group that is emptied goes; the view itself stays with its id, name and layout.
        let lost = views[3].as_object().unwrap();
        for key in [
            "dateBy",
            "endBy",
            "groupBy",
            "hiddenGroups",
            "sort",
            "filter",
        ] {
            assert!(!lost.contains_key(key), "{key} in {lost:?}");
        }
        assert_eq!(
            (
                lost["id"].as_str(),
                lost["name"].as_str(),
                lost["layout"].as_str()
            ),
            (Some("00000001"), Some("Lost"), Some("calendar"))
        );

        // `created` stays, `cardSize` is untouched, a `filter` that is no group stays as stored.
        assert_eq!(views[4]["dateBy"], json!("created"));
        assert_eq!(views[4]["cardSize"], json!("l"));
        assert_eq!(views[4]["filter"], json!("not a group"));
        // Entries that are no view stay as stored.
        assert_eq!(views[5], json!("not a view"));
        assert_eq!(views[6], json!(7));
        // The block's `source` is not touched.
        assert!(!changes.contains_key("source"));
    }

    #[test]
    fn a_view_that_is_the_mirror_of_the_block_keeps_both_in_step_when_the_filters_are_flat() {
        // A view written before the tree existed has a flat `filter` list under the old name only in the
        // block; the view's own `filter` is a tree. Both drop the same filters.
        let attrs = object(json!({
            "filters": [has_option(3, 32), is_set(4), is_set(2)],
            "views": [{ "id": "v", "name": "", "layout": "table", "properties": [],
                        "filter": { "op": "and", "items": [has_option(3, 32), is_set(4), is_set(2)] } }]
        }));
        let changes: HashMap<&str, Option<Value>> =
            remap_view(&attrs, &mapping()).into_iter().collect();
        let kept = Some(json!([{ "propertyId": pid(102), "op": "isSet" }]));
        assert_eq!(changes["filters"], kept);
        let Some(Some(Value::Array(views))) = changes.get("views") else {
            panic!("the views change");
        };
        assert_eq!(
            views[0]["filter"],
            json!({ "op": "and", "items": [{ "propertyId": pid(102), "op": "isSet" }] })
        );
    }

    #[test]
    fn a_view_filter_tree_that_loses_every_filter_goes() {
        let attrs = object(json!({
            "views": [{ "id": "v", "name": "", "layout": "list", "properties": [],
                        "filter": { "op": "or", "items": [is_set(4), { "op": "and", "items": [is_set(4)] }] } }]
        }));
        let changes: HashMap<&str, Option<Value>> =
            remap_view(&attrs, &mapping()).into_iter().collect();
        let Some(Some(Value::Array(views))) = changes.get("views") else {
            panic!("the views change");
        };
        assert!(views[0].get("filter").is_none());
    }

    /// The `views` of a block as `remap_view` writes them (the block holds one view, `view`).
    fn remapped_view(view: Value) -> Map<String, Value> {
        let attrs = object(json!({ "views": [view] }));
        let changes: HashMap<&str, Option<Value>> =
            remap_view(&attrs, &mapping()).into_iter().collect();
        let Some(Some(Value::Array(views))) = changes.get("views") else {
            panic!("the views change");
        };
        views[0].as_object().unwrap().clone()
    }

    #[test]
    fn the_sort_keys_of_a_view_follow_the_mapping_and_sort_follows_the_first_key_left() {
        let (d, p) = (pid(101), pid(103));
        // Every key has a counterpart (or is a built-in): each `by` follows and `sort` follows as it does alone.
        let view = remapped_view(json!({
            "id": "v", "sort": { "by": pid(1), "dir": "desc", "x": 1 },
            "sorts": [
                { "by": pid(1), "dir": "desc", "x": 1 },
                { "by": "created", "dir": "asc" },
                { "by": pid(3), "dir": "desc" },
                { "by": "title", "dir": "asc" }
            ]
        }));
        assert_eq!(
            view["sorts"],
            json!([
                { "by": d, "dir": "desc", "x": 1 },
                { "by": "created", "dir": "asc" },
                { "by": p, "dir": "desc" },
                { "by": "title", "dir": "asc" }
            ])
        );
        assert_eq!(view["sort"], json!({ "by": d, "dir": "desc", "x": 1 }));

        // A key with no counterpart goes; the others keep their order and `sort` stays the first.
        let view = remapped_view(json!({
            "id": "v", "sort": { "by": pid(1), "dir": "asc" },
            "sorts": [
                { "by": pid(1), "dir": "asc" }, { "by": pid(4), "dir": "desc" },
                { "by": "updated", "dir": "desc" }, { "by": pid(5), "dir": "asc" }
            ]
        }));
        assert_eq!(
            view["sorts"],
            json!([{ "by": d, "dir": "asc" }, { "by": "updated", "dir": "desc" }])
        );
        assert_eq!(view["sort"], json!({ "by": d, "dir": "asc" }));

        // The first key goes: `sort` follows the first one left.
        let view = remapped_view(json!({
            "id": "v", "sort": { "by": pid(4), "dir": "asc" },
            "sorts": [
                { "by": pid(4), "dir": "asc" }, { "by": pid(3), "dir": "desc" },
                { "by": "created", "dir": "asc" }
            ]
        }));
        assert_eq!(
            view["sorts"],
            json!([{ "by": p, "dir": "desc" }, { "by": "created", "dir": "asc" }])
        );
        assert_eq!(view["sort"], json!({ "by": p, "dir": "desc" }));

        // Every key goes: both go (the view then sorts by title, as a view without a sort does).
        let view = remapped_view(json!({
            "id": "v", "name": "kept", "sort": { "by": pid(4), "dir": "asc" },
            "sorts": [{ "by": pid(4), "dir": "asc" }, { "by": pid(5), "dir": "desc" }]
        }));
        assert!(
            !view.contains_key("sorts") && !view.contains_key("sort"),
            "{view:?}"
        );
        assert_eq!(view["name"], json!("kept"));

        // What this version does not understand stays: an empty list, entries that are no object or name
        // no `by`; a view without `sort` gets none.
        let view = remapped_view(json!({
            "id": "v",
            "sorts": ["junk", { "dir": "asc" }, { "by": pid(4), "dir": "asc" }, { "by": pid(1), "dir": "asc" }]
        }));
        assert_eq!(
            view["sorts"],
            json!(["junk", { "dir": "asc" }, { "by": d, "dir": "asc" }])
        );
        assert!(!view.contains_key("sort"));
        let view = remapped_view(json!({ "id": "v", "sorts": [], "properties": [pid(1)] }));
        assert_eq!(view["sorts"], json!([]));
        let view = remapped_view(json!({ "id": "v", "sorts": "title", "properties": [pid(1)] }));
        assert_eq!(view["sorts"], json!("title"));
    }

    #[test]
    fn the_new_filter_operators_follow_like_the_old_ones_and_relative_values_stay() {
        let (p, high) = (pid(103), oid(131));
        let view = remapped_view(json!({
            "id": "v",
            "filter": { "op": "and", "items": [
                { "propertyId": pid(3), "op": "lacksOption", "value": oid(31) },
                { "propertyId": pid(3), "op": "lacksOption", "value": oid(32) },
                { "propertyId": pid(3), "op": "lacksOption", "value": [oid(31), oid(32)] },
                { "propertyId": pid(3), "op": "lacksOption", "value": [oid(32)] },
                { "propertyId": pid(4), "op": "lacksOption", "value": oid(31) },
                { "propertyId": pid(3), "op": "notRelatesTo", "value": pid(9) },
                { "propertyId": pid(1), "op": "within", "value": { "relative": "nextDays", "n": 3 } },
                { "propertyId": pid(1), "op": "onOrAfter", "value": { "relative": "yesterday" } },
                { "propertyId": pid(1), "op": "notEquals", "value": 4 }
            ] }
        }));
        assert_eq!(
            view["filter"],
            json!({ "op": "and", "items": [
                { "propertyId": p, "op": "lacksOption", "value": high },
                { "propertyId": p, "op": "lacksOption", "value": [high] },
                { "propertyId": p, "op": "notRelatesTo", "value": pid(9) },
                { "propertyId": pid(101), "op": "within", "value": { "relative": "nextDays", "n": 3 } },
                { "propertyId": pid(101), "op": "onOrAfter", "value": { "relative": "yesterday" } },
                { "propertyId": pid(101), "op": "notEquals", "value": 4 }
            ] }),
            "a lacksOption with no option left goes; the others keep their values"
        );
    }

    #[test]
    fn the_linked_sources_are_kept_as_they_are() {
        for source in [
            json!({ "kind": "page", "pageId": pid(9) }),
            json!({ "kind": "space" }),
        ] {
            let attrs = object(json!({
                "source": source, "layout": "table", "properties": [pid(1)],
                "views": [{ "id": "v", "properties": [pid(1)] }]
            }));
            let changes: HashMap<&str, Option<Value>> =
                remap_view(&attrs, &mapping()).into_iter().collect();
            assert!(!changes.contains_key("source"), "{source}");
            // The ids of the views still follow.
            assert_eq!(changes["properties"], Some(json!([pid(101)])));
            let mut needs = Needs::default();
            needs.add_view(&attrs);
            assert_eq!(
                needs.props,
                BTreeSet::from([PropertyId::from_bytes([1; 16])]),
                "a page or space source asks nothing of the target schema"
            );
        }
    }

    #[test]
    fn the_target_schema_gets_what_the_sort_keys_and_the_new_filters_need() {
        let attrs = object(json!({
            "views": [{
                "id": "v", "name": "", "layout": "table", "properties": [],
                "sorts": [
                    { "by": pid(1), "dir": "asc" }, { "by": "title", "dir": "asc" },
                    { "by": pid(2), "dir": "desc" }, "junk", { "dir": "asc" }
                ],
                "filter": { "op": "and", "items": [
                    { "propertyId": pid(3), "op": "lacksOption", "value": [oid(31), oid(32)] },
                    { "propertyId": pid(5), "op": "notRelatesTo", "value": pid(9) }
                ] }
            }]
        }));
        let mut needs = Needs::default();
        needs.add_view(&attrs);
        let wanted: BTreeSet<PropertyId> = [1, 2, 3, 5]
            .into_iter()
            .map(|n| PropertyId::from_bytes([n; 16]))
            .collect();
        assert_eq!(needs.props, wanted);
        assert_eq!(needs.views, wanted);
        assert_eq!(
            needs.options[&PropertyId::from_bytes([3; 16])],
            BTreeSet::from([31, 32].map(|n| OptionId::from_bytes([n; 16])))
        );
    }

    #[test]
    fn views_that_map_to_themselves_change_nothing() {
        let attrs = block_with_views();
        let mut identity = SchemaMapping::default();
        for n in [1, 2, 3, 4] {
            identity.props.insert(
                PropertyId::from_bytes([n; 16]),
                PropertyId::from_bytes([n; 16]),
            );
        }
        // Every option the fixture names (the "Lost" view hides option 31 of property 4).
        for (property, options) in [(3, [31, 32].as_slice()), (4, [31].as_slice())] {
            identity.options.insert(
                PropertyId::from_bytes([property; 16]),
                options
                    .iter()
                    .map(|n| {
                        (
                            OptionId::from_bytes([*n; 16]),
                            OptionId::from_bytes([*n; 16]),
                        )
                    })
                    .collect(),
            );
        }
        assert!(remap_view(&attrs, &identity).is_empty());
    }

    #[test]
    fn the_target_schema_gets_what_every_view_needs() {
        // Nothing in the legacy attributes: the views alone name the properties and options.
        let attrs = object(json!({
            "views": [
                { "id": "a", "name": "", "layout": "calendar", "dateBy": pid(1), "endBy": pid(2), "properties": [] },
                { "id": "b", "name": "", "layout": "calendar", "dateBy": "created" },
                { "id": "c", "name": "", "layout": "table", "properties": [pid(5)],
                  "filter": { "op": "and", "items": [
                      has_option(3, 31),
                      { "op": "or", "items": [
                          { "propertyId": pid(3), "op": "hasOption", "value": [oid(32), oid(33)] },
                          is_set(6)] }] },
                  "groupBy": pid(7), "hiddenGroups": [oid(71)] },
                "garbage",
                { "id": 5, "properties": "x", "filter": { "items": 3 } }
            ]
        }));
        let mut needs = Needs::default();
        needs.add_view(&attrs);
        let wanted: BTreeSet<PropertyId> = [1, 2, 3, 5, 6, 7]
            .into_iter()
            .map(|n| PropertyId::from_bytes([n; 16]))
            .collect();
        assert_eq!(needs.props, wanted);
        assert_eq!(
            needs.views, wanted,
            "a computed property is created for a view"
        );
        assert_eq!(
            needs.options[&PropertyId::from_bytes([3; 16])],
            BTreeSet::from([31, 32, 33].map(|n| OptionId::from_bytes([n; 16])))
        );
        assert_eq!(
            needs.options[&PropertyId::from_bytes([7; 16])],
            BTreeSet::from([OptionId::from_bytes([71; 16])])
        );
    }

    #[test]
    fn a_stored_view_of_any_shape_never_panics_the_remap() {
        let mut deep = json!({ "op": "and", "items": [] });
        for _ in 0..300 {
            deep = json!({ "op": "or", "items": [deep, is_set(1)] });
        }
        for views in [
            json!("views"),
            json!(null),
            json!([null, [], {}, { "filter": deep.clone(), "properties": 5, "sort": [], "dateBy": 4 }]),
            json!([{ "id": "v", "filter": deep, "columnWidths": [], "calculations": 3, "groupBy": 9,
                     "hiddenGroups": "x", "endBy": [], "dateBy": {} }]),
        ] {
            let attrs = object(json!({ "views": views, "filters": 4, "filter": [], "dateBy": 1 }));
            let _ = remap_view(&attrs, &mapping());
            let mut needs = Needs::default();
            needs.add_view(&attrs);
        }
    }

    // ----- dashboards and formulas (ADR-0055) --------------------------------------------------

    /// A dashboard view of the design document: a number over a property, a chart grouped by one and
    /// measured over another, a chart that groups by a page time, a number with its own filter, and an
    /// embedded view; with what has no counterpart (property 4, option 32) in the places that name them.
    fn dashboard() -> Map<String, Value> {
        object(json!({
            "views": [
                { "id": "d1", "name": "Overview", "layout": "dashboard", "properties": [],
                  "filter": { "op": "and", "items": [is_set(3)] },
                  "widgets": [
                    { "id": "w0000001", "kind": "number", "title": "Hours", "w": 1, "h": "s",
                      "calc": { "by": pid(1), "fn": "sum" } },
                    { "id": "w0000002", "kind": "number", "title": "", "w": 1, "h": "s",
                      "calc": { "by": "title", "fn": "count_all" },
                      "filter": { "op": "and", "items": [has_option(3, 31), is_set(4)] } },
                    { "id": "w0000003", "kind": "chart", "title": "", "w": 2, "h": "m",
                      "chart": { "type": "bar", "groupBy": pid(3),
                                 "measure": { "fn": "sum", "propertyId": pid(1) },
                                 "bucket": "week", "futureChartKey": [pid(1)] } },
                    { "id": "w0000004", "kind": "chart", "title": "Created", "w": 2, "h": "m",
                      "chart": { "type": "line", "groupBy": "created", "bucket": "month",
                                 "measure": { "fn": "count" } } },
                    { "id": "w0000005", "kind": "number", "title": "Gone", "w": 1, "h": "s",
                      "calc": { "by": pid(4), "fn": "sum" },
                      "filter": { "op": "or", "items": [is_set(4)] } },
                    { "id": "w0000006", "kind": "chart", "title": "Lost", "w": 2, "h": "m",
                      "chart": { "type": "donut", "groupBy": pid(4),
                                 "measure": { "fn": "average", "propertyId": pid(4) } } },
                    { "id": "w0000007", "kind": "view", "title": "", "w": 4, "h": "l", "viewId": "k3f9a2bc" },
                    { "id": "w0000008", "kind": "newer", "unknownKey": { "ids": [pid(1)] } },
                    "not a widget"
                  ] }
            ]
        }))
    }

    #[test]
    fn the_widgets_of_a_dashboard_follow_the_mapping_and_lose_what_has_no_counterpart() {
        let attrs = dashboard();
        let changes: HashMap<&str, Option<Value>> =
            remap_view(&attrs, &mapping()).into_iter().collect();
        let d = PropertyId::from_bytes([101; 16]).to_string();
        let p = PropertyId::from_bytes([103; 16]).to_string();
        let high = OptionId::from_bytes([131; 16]).to_string();
        let Some(Some(Value::Array(views))) = changes.get("views") else {
            panic!("the views change");
        };
        // The view's own filter follows as a view's does.
        assert_eq!(
            views[0]["filter"],
            json!({ "op": "and", "items": [{ "propertyId": p, "op": "isSet" }] })
        );
        let widgets = views[0]["widgets"].as_array().unwrap();
        assert_eq!(widgets.len(), 9);
        // A number's column follows; the rest of the widget is as it was.
        assert_eq!(widgets[0]["calc"], json!({ "by": d, "fn": "sum" }));
        assert_eq!(
            (widgets[0]["id"].as_str(), widgets[0]["title"].as_str()),
            (Some("w0000001"), Some("Hours"))
        );
        // `title` stays; the widget's own filter follows, what has no counterpart goes.
        assert_eq!(
            widgets[1]["calc"],
            json!({ "by": "title", "fn": "count_all" })
        );
        assert_eq!(
            widgets[1]["filter"],
            json!({ "op": "and", "items": [{ "propertyId": p, "op": "hasOption", "value": high }] })
        );
        // A chart: the grouping, the measure, and the keys this version does not know.
        assert_eq!(
            widgets[2]["chart"],
            json!({ "type": "bar", "groupBy": p, "measure": { "fn": "sum", "propertyId": d },
                    "bucket": "week", "futureChartKey": [pid(1)] })
        );
        // `created` stays.
        assert_eq!(widgets[3], dashboard()["views"][0]["widgets"][3]);
        // No counterpart: the number counts all pages, its filter (emptied) goes; the chart asks to choose
        // and counts.
        assert_eq!(
            widgets[4]["calc"],
            json!({ "by": "title", "fn": "count_all" })
        );
        assert!(widgets[4].get("filter").is_none());
        assert_eq!(
            widgets[5]["chart"],
            json!({ "type": "donut", "groupBy": null, "measure": { "fn": "count" } })
        );
        // The embedded view, an unknown kind and an entry that is no widget are untouched.
        assert_eq!(widgets[6], dashboard()["views"][0]["widgets"][6]);
        assert_eq!(widgets[7], dashboard()["views"][0]["widgets"][7]);
        assert_eq!(widgets[8], json!("not a widget"));
        // The block carries no widgets of its own.
        assert!(!changes.contains_key("widgets"));
    }

    #[test]
    fn the_widgets_of_a_dashboard_need_their_properties_and_options_in_the_target() {
        let mut needs = Needs::default();
        needs.add_view(&dashboard());
        let wanted: BTreeSet<PropertyId> = [1, 3, 4]
            .into_iter()
            .map(|n| PropertyId::from_bytes([n; 16]))
            .collect();
        assert_eq!(needs.props, wanted, "calc, groupBy, measure and filters");
        assert_eq!(
            needs.options[&PropertyId::from_bytes([3; 16])],
            BTreeSet::from([OptionId::from_bytes([31; 16])]),
            "the option of a widget's own filter"
        );
    }

    #[test]
    fn a_dashboard_that_maps_to_itself_changes_nothing() {
        let mut identity = SchemaMapping::default();
        for n in [1, 3, 4] {
            identity.props.insert(
                PropertyId::from_bytes([n; 16]),
                PropertyId::from_bytes([n; 16]),
            );
        }
        identity.options.insert(
            PropertyId::from_bytes([3; 16]),
            HashMap::from([(
                OptionId::from_bytes([31; 16]),
                OptionId::from_bytes([31; 16]),
            )]),
        );
        assert!(remap_view(&dashboard(), &identity).is_empty());
    }

    fn formula_def(id: u8, name: &str, expression: String) -> PropertyDef {
        PropertyDef {
            config: Some(PropertyConfig::Formula {
                expression,
                result_type: None,
            }),
            ..def(id, name, PropertyKind::Formula)
        }
    }

    #[test]
    fn a_formula_a_view_names_is_made_in_the_target_after_what_it_reads() {
        let rng = SeededRng::new(11);
        // Score reads Priority (number); Total reads Score and the title.
        let src = vec![
            tags(),
            def(1, "Priority", PropertyKind::Number),
            formula_def(2, "Score", format!("prop(\"{}\") * 10", pid(1))),
            formula_def(
                3,
                "Total",
                format!("prop(\"{}\") + length(prop(\"title\"))", pid(2)),
            ),
            def(4, "Unused", PropertyKind::Text),
        ];
        // A view names Total only (a column of its table, a widget of a dashboard).
        let attrs = object(json!({
            "views": [
                { "id": "t", "name": "", "layout": "table", "properties": [pid(3)] },
                { "id": "d", "name": "", "layout": "dashboard", "widgets": [
                    { "id": "w", "kind": "number", "calc": { "by": pid(3), "fn": "sum" } }] }
            ]
        }));
        let mut needs = Needs::default();
        needs.add_view(&attrs);
        needs.add_formula_reads(&src);
        let wanted: BTreeSet<PropertyId> = [1, 2, 3]
            .into_iter()
            .map(|n| PropertyId::from_bytes([n; 16]))
            .collect();
        assert_eq!(needs.props, wanted, "what Total reads, through Score");
        assert!(!needs.props.contains(&PropertyId::from_bytes([4; 16])));

        let mapping = plan_schema(&src, &[tags()], &needs, &rng);
        assert_eq!(mapping.created, 3);
        let made: Vec<(&str, PropertyKind)> = mapping
            .ops
            .iter()
            .filter_map(|op| match op {
                SchemaOp::Property { name, kind, .. } => Some((name.as_str(), *kind)),
                _ => None,
            })
            .collect();
        assert_eq!(
            made,
            [
                ("Priority", PropertyKind::Number),
                ("Score", PropertyKind::Formula),
                ("Total", PropertyKind::Formula)
            ],
            "a formula after what it reads"
        );
        let target_id = |source: u8| mapping.props[&PropertyId::from_bytes([source; 16])];
        let expression_of = |source: u8| {
            let def = mapping
                .target
                .iter()
                .find(|def| def.id == target_id(source))
                .unwrap();
            match &def.config {
                Some(PropertyConfig::Formula { expression, .. }) => expression.clone(),
                other => panic!("not a formula: {other:?}"),
            }
        };
        assert_eq!(expression_of(2), format!("prop(\"{}\") * 10", target_id(1)));
        assert_eq!(
            expression_of(3),
            format!("prop(\"{}\") + length(prop(\"title\"))", target_id(2)),
            "the title stays the title"
        );
    }

    #[test]
    fn a_formula_of_the_same_name_is_used_and_one_that_reads_a_stranger_stays_as_written() {
        let rng = SeededRng::new(12);
        let src = vec![
            tags(),
            def(1, "Priority", PropertyKind::Number),
            formula_def(2, "Score", format!("prop(\"{}\") * 10", pid(1))),
            formula_def(3, "Orphan", format!("prop(\"{}\") + 1", pid(99))),
        ];
        let mut needs = Needs::default();
        needs.add_view(&object(json!({ "properties": [pid(2), pid(3)] })));
        needs.add_formula_reads(&src);
        // The target already has a formula named Score (whatever it says).
        let dst = vec![
            tags(),
            def(11, "Priority", PropertyKind::Number),
            formula_def(12, "score", "1".to_owned()),
        ];
        let mapping = plan_schema(&src, &dst, &needs, &rng);
        assert_eq!(
            mapping.props[&PropertyId::from_bytes([2; 16])],
            PropertyId::from_bytes([12; 16]),
            "bound by name and kind"
        );
        let orphan = mapping.props[&PropertyId::from_bytes([3; 16])];
        let created = mapping.target.iter().find(|def| def.id == orphan).unwrap();
        assert_eq!(
            created.config,
            Some(PropertyConfig::Formula {
                expression: format!("prop(\"{}\") + 1", pid(99)),
                result_type: None
            }),
            "an id with no counterpart stays as it is"
        );
    }
}
