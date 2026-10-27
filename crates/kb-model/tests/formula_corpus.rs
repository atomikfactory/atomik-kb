//! The shared corpus of formula cases (`docs/architecture/formulas.md` section 7.3): every
//! `tests/formula-cases/*.json` file, sorted by name. The TypeScript twin runs the same files.
//!
//! A file holds a `schema`, `pages`, `people`, `files`, `env`, a `titleLabel` and the `cases`.
//! Ids in the files are short strings; the harness gives each a real id derived from it (and
//! turns page and person ids of a result back). Expressions are in display form: the harness
//! parses them (a syntax error is reported on the display text), converts them with `to_stored`
//! (an unknown property is reported on the display text), then parses and checks the stored text
//! (the other errors are reported on the stored text, where an id is longer than a name), and
//! evaluates when the case has a `result`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr
)]

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use kb_model::formula::{self, FormulaEnv, FormulaHost, FormulaProperty, FormulaSchema};
use kb_types::properties::FORMULA_TITLE_ID;
use kb_types::{
    AccountId, AttachmentId, OptionId, PageId, PropertyId, PropertyKind, PropertyValue,
    RollupFunction,
};
use serde_json::{Value, json};

/// A deterministic 16-byte id for a short id of a namespace.
fn derive(namespace: &str, short: &str) -> [u8; 16] {
    let mut out = [0_u8; 16];
    for (half, seed) in [0xcbf2_9ce4_8422_2325_u64, 0x8422_2325_cbf2_9ce4_u64]
        .into_iter()
        .enumerate()
    {
        let mut hash = seed;
        for byte in namespace.bytes().chain([0]).chain(short.bytes()) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        out[half * 8..half * 8 + 8].copy_from_slice(&hash.to_be_bytes());
    }
    out
}

/// Days since 1970-01-01 of a civil date (written out here so the harness does not lean on the
/// engine's own calendar).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let month_from_march = (month + 9) % 12;
    let day_of_year = (153 * month_from_march + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// A local time `YYYY-MM-DDTHH:mm[:ss]` as Unix ms, given the offset east of UTC in minutes.
fn unix_ms(local: &str, offset_minutes: i64) -> i64 {
    let (day, time) = local.split_once('T').unwrap_or((local, "00:00"));
    let mut date = day.split('-').map(|part| part.parse::<i64>().unwrap());
    let (year, month, d) = (
        date.next().unwrap(),
        date.next().unwrap(),
        date.next().unwrap(),
    );
    let mut clock = time.split(':').map(|part| part.parse::<i64>().unwrap());
    let (hour, minute, second) = (
        clock.next().unwrap_or(0),
        clock.next().unwrap_or(0),
        clock.next().unwrap_or(0),
    );
    let local_ms = days_from_civil(year, month, d) * 86_400_000
        + hour * 3_600_000
        + minute * 60_000
        + second * 1000;
    local_ms - offset_minutes * 60_000
}

struct PageData {
    title: String,
    created: Option<i64>,
    edited: Option<i64>,
    values: HashMap<PropertyId, PropertyValue>,
}

/// The schema, pages, people and files of one corpus file.
struct World {
    props: Vec<FormulaProperty>,
    pages: HashMap<PageId, PageData>,
    first_page: Option<PageId>,
    people: HashMap<AccountId, (String, String)>,
    files: HashMap<AttachmentId, String>,
    /// A real id (as text) back to its short id: pages and people.
    short: HashMap<String, String>,
    env: FormulaEnv,
    title_label: String,
    /// Short property ids to real ones.
    property_ids: HashMap<String, PropertyId>,
}

fn string_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

impl World {
    fn load(file: &Value) -> World {
        let offset_minutes = file["env"]["offsetMinutes"].as_i64().unwrap_or(0);
        let now = string_of(&file["env"], "now").map_or(0, |now| unix_ms(now, offset_minutes));
        let mut world = World {
            props: Vec::new(),
            pages: HashMap::new(),
            first_page: None,
            people: HashMap::new(),
            files: HashMap::new(),
            short: HashMap::new(),
            env: FormulaEnv {
                now_ms: now,
                offset_seconds: i32::try_from(offset_minutes * 60).unwrap(),
            },
            title_label: string_of(file, "titleLabel").unwrap_or("Name").to_owned(),
            property_ids: HashMap::new(),
        };
        for person in file["people"].as_array().into_iter().flatten() {
            let short = string_of(person, "id").unwrap();
            let id = AccountId::from_random_bytes(derive("person", short));
            world.short.insert(id.to_string(), short.to_owned());
            world.people.insert(
                id,
                (
                    string_of(person, "name").unwrap_or("").to_owned(),
                    string_of(person, "email").unwrap_or("").to_owned(),
                ),
            );
        }
        for entry in file["files"].as_array().into_iter().flatten() {
            let id =
                AttachmentId::from_random_bytes(derive("file", string_of(entry, "id").unwrap()));
            world
                .files
                .insert(id, string_of(entry, "name").unwrap_or("").to_owned());
        }
        world.load_schema(file);
        world.load_pages(file);
        world
    }

    fn load_schema(&mut self, file: &Value) {
        let mut displays: Vec<(usize, String)> = Vec::new();
        for entry in file["schema"].as_array().into_iter().flatten() {
            let short = string_of(entry, "id").unwrap();
            let id = PropertyId::from_random_bytes(derive("property", short));
            self.property_ids.insert(short.to_owned(), id);
            let kind = PropertyKind::parse(string_of(entry, "kind").unwrap()).unwrap();
            let options = entry["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|option| {
                    (
                        OptionId::from_random_bytes(derive(
                            "option",
                            string_of(option, "id").unwrap(),
                        )),
                        string_of(option, "name").unwrap().to_owned(),
                    )
                })
                .collect();
            let rollup = entry.get("rollup").map(|rollup| {
                (
                    RollupFunction::parse(string_of(rollup, "function").unwrap()).unwrap(),
                    string_of(rollup, "targetKind").map(|kind| PropertyKind::parse(kind).unwrap()),
                )
            });
            if let Some(expression) = string_of(entry, "expression") {
                displays.push((self.props.len(), expression.to_owned()));
            }
            self.props.push(FormulaProperty {
                id: id.to_string(),
                name: string_of(entry, "name").unwrap().to_owned(),
                kind,
                options,
                expression: None,
                rollup,
            });
        }
        // A formula's expression is in display form in the file; the schema holds it stored.
        for (index, display) in displays {
            let stored = formula::to_stored(&display, &|name| self.id_of_name(name))
                .unwrap_or_else(|_| display.clone());
            self.props[index].expression = Some(stored);
        }
    }

    /// The id a property name stands for: the schema's properties in order, then the title.
    fn id_of_name(&self, name: &str) -> Option<String> {
        self.props
            .iter()
            .find(|property| property.name == name)
            .map(|property| property.id.clone())
            .or_else(|| {
                (name == "title" || name == self.title_label).then(|| FORMULA_TITLE_ID.to_owned())
            })
    }

    fn load_pages(&mut self, file: &Value) {
        let offset_minutes = file["env"]["offsetMinutes"].as_i64().unwrap_or(0);
        for entry in file["pages"].as_array().into_iter().flatten() {
            let short = string_of(entry, "id").unwrap();
            let id = PageId::from_random_bytes(derive("page", short));
            self.short.insert(id.to_string(), short.to_owned());
            self.first_page.get_or_insert(id);
            let mut values = HashMap::new();
            if let Some(map) = entry["values"].as_object() {
                for (property, value) in map {
                    let pid = *self.property_ids.get(property).unwrap();
                    values.insert(pid, Self::property_value(value));
                }
            }
            self.pages.insert(
                id,
                PageData {
                    title: string_of(entry, "title").unwrap_or("").to_owned(),
                    created: string_of(entry, "created").map(|t| unix_ms(t, offset_minutes)),
                    edited: string_of(entry, "edited").map(|t| unix_ms(t, offset_minutes)),
                    values,
                },
            );
        }
    }

    /// Replaces the short ids of a stored value's JSON by real ones, then reads it.
    fn property_value(value: &Value) -> PropertyValue {
        let mut json = value.clone();
        Self::real_ids(&mut json);
        serde_json::from_value(json).unwrap()
    }

    fn real_ids(value: &mut Value) {
        let kind = value["kind"].as_str().unwrap_or("").to_owned();
        let map_all = |value: &mut Value, namespace: &str| match value {
            Value::String(short) => {
                let real = uuid_text(namespace, short);
                *short = real;
            }
            Value::Array(items) => {
                for item in items {
                    if let Value::String(short) = item {
                        let real = uuid_text(namespace, short);
                        *short = real;
                    }
                }
            }
            _ => {}
        };
        match kind.as_str() {
            "select" | "status" | "multi_select" => map_all(&mut value["value"], "option"),
            "relation" => map_all(&mut value["value"], "page"),
            "person" => map_all(&mut value["value"], "person"),
            "files" => map_all(&mut value["value"], "file"),
            "checklist" => {
                if let Some(items) = value["value"].as_array_mut() {
                    for (index, item) in items.iter_mut().enumerate() {
                        if item.get("id").is_none() {
                            item["id"] = Value::String(uuid_text("item", &index.to_string()));
                        } else if let Some(short) = item["id"].as_str().map(str::to_owned) {
                            item["id"] = Value::String(uuid_text("item", &short));
                        }
                    }
                }
            }
            "rollup" => {
                if value["value"]["result"]["type"] == "values"
                    && let Some(items) = value["value"]["result"]["data"].as_array_mut()
                {
                    for item in items {
                        Self::real_ids(item);
                    }
                }
            }
            _ => {}
        }
    }

    fn property_id(&self, short: &str) -> PropertyId {
        *self
            .property_ids
            .get(short)
            .unwrap_or_else(|| panic!("no property \"{short}\" in the file"))
    }

    /// The JSON of a result with its page and person ids turned back into short ids.
    fn short_ids(&self, value: &mut Value) {
        if let Some(kind) = value["type"].as_str().map(str::to_owned) {
            match kind.as_str() {
                "page" | "person" => {
                    if let Some(real) = value["data"].as_str()
                        && let Some(short) = self.short.get(real)
                    {
                        value["data"] = Value::String(short.clone());
                    }
                }
                "list" => {
                    if let Some(items) = value["data"].as_array_mut() {
                        for item in items {
                            self.short_ids(item);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn uuid_text(namespace: &str, short: &str) -> String {
    // The version and variant bits make it a valid UUID text; every id type parses the same text.
    PageId::from_random_bytes(derive(namespace, short)).to_string()
}

impl FormulaSchema for World {
    fn property(&self, id: &str) -> Option<FormulaProperty> {
        self.props
            .iter()
            .find(|property| property.id == id)
            .cloned()
    }
}

impl FormulaHost for World {
    fn title(&self, page: PageId) -> Option<String> {
        self.pages.get(&page).map(|data| data.title.clone())
    }

    fn value(&self, page: PageId, property: PropertyId) -> Option<PropertyValue> {
        self.pages.get(&page)?.values.get(&property).cloned()
    }

    fn related(&self, page: PageId, relation: PropertyId) -> Vec<PageId> {
        match self
            .pages
            .get(&page)
            .and_then(|data| data.values.get(&relation))
        {
            Some(PropertyValue::Relation(pages)) => pages.clone(),
            _ => Vec::new(),
        }
    }

    fn created_ms(&self, page: PageId) -> Option<i64> {
        self.pages.get(&page)?.created
    }

    fn edited_ms(&self, page: PageId) -> Option<i64> {
        self.pages.get(&page)?.edited
    }

    fn person(&self, account: AccountId) -> Option<(String, String)> {
        self.people.get(&account).cloned()
    }

    fn file_name(&self, attachment: AttachmentId) -> Option<String> {
        self.files.get(&attachment).cloned()
    }
}

/// Numbers compare exactly unless the case has a relative tolerance.
fn same(expected: &Value, actual: &Value, tolerance: Option<f64>) -> bool {
    match (expected, actual) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            match tolerance {
                Some(tolerance) => (x - y).abs() <= tolerance * x.abs().max(y.abs()),
                None => x == y,
            }
        }
        (Value::Array(xs), Value::Array(ys)) => {
            xs.len() == ys.len() && xs.iter().zip(ys).all(|(x, y)| same(x, y, tolerance))
        }
        (Value::Object(xs), Value::Object(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .all(|(key, x)| ys.get(key).is_some_and(|y| same(x, y, tolerance)))
        }
        _ => expected == actual,
    }
}

fn error_json(error: &formula::FormulaError) -> String {
    format!(
        "{} at {}..{} ({})",
        error.code.as_str(),
        error.start,
        error.end,
        error.message
    )
}

/// Checks that an error is the one the case expects.
fn expect_error(case: &Value, error: &formula::FormulaError) -> Result<(), String> {
    let Some(code) = string_of(case, "error") else {
        return Err(format!("unexpected error: {}", error_json(error)));
    };
    if error.code.as_str() != code {
        return Err(format!("expected error {code}, got {}", error_json(error)));
    }
    if let Some(at) = case.get("at").and_then(Value::as_u64)
        && u64::try_from(error.start).ok() != Some(at)
    {
        return Err(format!(
            "expected error {code} at {at}, got {}",
            error_json(error)
        ));
    }
    Ok(())
}

fn run_case(world: &World, case: &Value) -> Result<(), String> {
    let display = string_of(case, "expression").ok_or("no expression")?;
    // The grammar is the same in both forms: a syntax error is reported on what the person wrote.
    if let Err(error) = formula::parse(display) {
        return expect_error(case, &error);
    }
    let stored = match formula::to_stored(display, &|name| world.id_of_name(name)) {
        Ok(stored) => stored,
        Err(error) => return expect_error(case, &error),
    };
    let parsed = match formula::parse(&stored) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Err(format!(
                "the stored text does not parse: {}",
                error_json(&error)
            ));
        }
    };
    let self_id = string_of(case, "self").map(|short| world.property_id(short).to_string());
    let checked = match formula::check(&parsed, world, self_id.as_deref()) {
        Ok(checked) => checked,
        Err(error) => return expect_error(case, &error),
    };
    if let Some(code) = string_of(case, "error") {
        return Err(format!(
            "expected error {code}, but it checks as {checked:?}"
        ));
    }
    if let Some(expected) = case.get("type") {
        let actual = serde_json::to_value(checked).unwrap();
        if &actual != expected {
            return Err(format!("expected type {expected}, got {actual}"));
        }
    }
    let Some(expected) = case.get("result") else {
        return Ok(());
    };
    let page = match string_of(case, "page") {
        Some(short) => PageId::from_random_bytes(derive("page", short)),
        None => world.first_page.ok_or("the file has no page")?,
    };
    let evaluated = formula::evaluate(&parsed, page, world, &world.env);
    let mut actual = match &evaluated {
        Some(result) => serde_json::to_value(result).unwrap(),
        None => Value::Null,
    };
    world.short_ids(&mut actual);
    let tolerance = case.get("tolerance").and_then(Value::as_f64);
    if same(expected, &actual, tolerance) {
        Ok(())
    } else {
        Err(format!("expected {expected}, got {actual}"))
    }
}

fn files() -> Vec<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/formula-cases");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    files.sort();
    files
}

#[test]
fn every_corpus_case_passes() {
    let files = files();
    assert!(!files.is_empty(), "no corpus files");
    let (mut total, mut failures) = (0_usize, Vec::new());
    for path in &files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let file: Value = serde_json::from_str(&fs::read_to_string(path).unwrap())
            .unwrap_or_else(|error| panic!("{name} is not JSON: {error}"));
        let world = World::load(&file);
        let cases = file["cases"]
            .as_array()
            .unwrap_or_else(|| panic!("{name} has no cases"));
        assert!(!cases.is_empty(), "{name} has no cases");
        for case in cases {
            total += 1;
            let case_name = string_of(case, "name").unwrap_or("(unnamed)");
            if let Err(message) = run_case(&world, case) {
                failures.push(format!("{name}: {case_name}: {message}"));
            }
        }
    }
    eprintln!(
        "formula corpus: {} of {total} cases pass in {} files",
        total - failures.len(),
        files.len()
    );
    assert!(
        failures.is_empty(),
        "{} of {total} corpus cases fail:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn the_harness_itself_sees_the_cases_it_should() {
    // A guard against a harness that silently runs nothing: the contract file is there with its
    // 100 and more cases, and a wrong expectation is reported.
    let contract: Value = serde_json::from_str(
        &fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/formula-cases/00-contract.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(contract["cases"].as_array().unwrap().len() >= 100);
    let world = World::load(&contract);
    let wrong = json!({ "name": "wrong", "expression": "1 + 1", "result": { "type": "number", "data": 3 } });
    assert!(run_case(&world, &wrong).is_err());
    let wrong_error = json!({ "name": "wrong", "expression": "1 +", "error": "type" });
    assert!(run_case(&world, &wrong_error).is_err());
    let missing_error = json!({ "name": "wrong", "expression": "1 + 1", "error": "type" });
    assert!(run_case(&world, &missing_error).is_err());
    assert!(same(&json!(1.0), &json!(1.0 + 1e-13), Some(1e-12)));
    assert!(!same(&json!(1.0), &json!(1.0 + 1e-13), None));
}
