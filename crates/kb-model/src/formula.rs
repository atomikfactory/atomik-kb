//! Formulas (ADR-0055, `docs/architecture/formulas.md`): a Notion 2.0 style expression language over a page's
//! properties and the pages of its relations. Pure: no clock, no I/O; the caller (`StructureDoc::values_of`, the
//! property writes) gives the schema, the values and the time through [`FormulaHost`] and [`FormulaEnv`]. The
//! TypeScript twin (`core-client/src/formula/`) computes the same results; both run the shared corpus of
//! `crates/kb-model/tests/formula-cases/`.
//!
//! The engine lives in submodules: `lexer` and `parser` (text to the syntax tree of `ast`), `check` (the strict
//! types, the scopes, the cycles; it builds the typed tree of `types` the evaluator runs), `eval` and `builtins`
//! (the evaluator and the functions of section 3 over `value`s), `dates`, `format` and `regex_js` (the calendar,
//! numbers as text and JavaScript's regular expressions on the `regex` crate) and `convert` (names and ids).

use std::sync::{Arc, OnceLock};

use kb_types::{
    AccountId, AttachmentId, FormulaResult, FormulaType, OptionId, PageId, PropertyId,
    PropertyKind, PropertyValue, RollupFunction,
};

mod ast;
mod builtins;
mod check;
mod convert;
mod dates;
mod eval;
mod format;
mod functions;
mod lexer;
mod parser;
mod regex_js;
#[cfg(test)]
mod tests;
mod types;
mod value;

/// The time of a read: Unix ms and the device's offset from UTC (`Clock::local_offset_seconds`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormulaEnv {
    /// `Clock::now_ms` at the read.
    pub now_ms: i64,
    /// `Clock::local_offset_seconds` at the read.
    pub offset_seconds: i32,
}

/// What went wrong in a formula (section 4.5 of the design).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FormulaErrorCode {
    /// The text does not parse.
    Syntax,
    /// Longer than `MAX_FORMULA_CHARS` or deeper than `MAX_FORMULA_DEPTH`.
    TooLong,
    /// `prop("…")` names no property.
    UnknownProperty,
    /// A `created_by` or `last_edited_by` property.
    UnsupportedProperty,
    /// No function of that name.
    UnknownFunction,
    /// No variable of that name.
    UnknownVariable,
    /// A wrong number of arguments.
    Arity,
    /// A value of the wrong type.
    Type,
    /// The formula depends on itself.
    Cycle,
}

impl FormulaErrorCode {
    /// The code's name in the API and the corpus (`syntax`, `too_long`, …).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            FormulaErrorCode::Syntax => "syntax",
            FormulaErrorCode::TooLong => "too_long",
            FormulaErrorCode::UnknownProperty => "unknown_property",
            FormulaErrorCode::UnsupportedProperty => "unsupported_property",
            FormulaErrorCode::UnknownFunction => "unknown_function",
            FormulaErrorCode::UnknownVariable => "unknown_variable",
            FormulaErrorCode::Arity => "arity",
            FormulaErrorCode::Type => "type",
            FormulaErrorCode::Cycle => "cycle",
        }
    }
}

/// A parse or check error: the code, an English message, the range at fault (characters, `end`
/// exclusive) and a detail for the UI's translated message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaError {
    /// What went wrong.
    pub code: FormulaErrorCode,
    /// English, for logs and the API.
    pub message: String,
    /// First character at fault.
    pub start: usize,
    /// One past the last character at fault.
    pub end: usize,
    /// The detail of the code (section 4.5): a name, `"f:min:max"`, `"expected:found"`…
    pub detail: String,
}

impl std::fmt::Display for FormulaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} at {}..{}: {}",
            self.code.as_str(),
            self.start,
            self.end,
            self.message
        )
    }
}

/// A property as a formula sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaProperty {
    /// The id as the stored expression writes it (`PropertyId` text).
    pub id: String,
    /// The display name.
    pub name: String,
    /// The kind.
    pub kind: PropertyKind,
    /// The options of a select, multi-select or status: id and name.
    pub options: Vec<(OptionId, String)>,
    /// A formula's stored expression.
    pub expression: Option<String>,
    /// A rollup's function and the kind of its target (`None` = the title).
    pub rollup: Option<(RollupFunction, Option<PropertyKind>)>,
}

/// The schema a formula is checked against: properties by their stored id (never `"title"`, which the engine
/// knows itself).
pub trait FormulaSchema {
    /// The property with this id.
    fn property(&self, id: &str) -> Option<FormulaProperty>;
}

/// The pages a formula reads.
pub trait FormulaHost: FormulaSchema {
    /// The page's title (`None` for an unknown page).
    fn title(&self, page: PageId) -> Option<String>;
    /// A stored value or a rollup's value (never a formula's: the engine evaluates formulas itself).
    fn value(&self, page: PageId, property: PropertyId) -> Option<PropertyValue>;
    /// The live related pages of a relation property, in order (both directions, as the rollup reads them).
    fn related(&self, page: PageId, relation: PropertyId) -> Vec<PageId>;
    /// When the page was created, Unix ms.
    fn created_ms(&self, page: PageId) -> Option<i64>;
    /// When the page was last edited, Unix ms.
    fn edited_ms(&self, page: PageId) -> Option<i64>;
    /// A member's name and e-mail.
    fn person(&self, account: AccountId) -> Option<(String, String)>;
    /// An attachment's file name.
    fn file_name(&self, attachment: AttachmentId) -> Option<String>;
}

/// A parsed expression.
///
/// The first [`evaluate`] checks the expression against the schema it is given and keeps the checked
/// tree (and those of the formula properties it reads) inside the `Formula`; later evaluations only
/// run it. **The cache assumes that a `Formula` is evaluated against one schema**: the owner of the
/// schema (`StructureDoc`'s `read_schema`) rebuilds its parsed formulas whenever the schema changes,
/// so the cache lives exactly as long as it is valid. A `Formula` evaluated against another schema
/// must be parsed again. [`check`] never uses the cache. `Formula` is `Send` and `Sync`, and a clone
/// made after the first evaluation keeps what is cached.
#[derive(Clone)]
pub struct Formula {
    ast: ast::Expr,
    /// The checked tree for [`evaluate`] (`None`: the expression does not check).
    compiled: OnceLock<Option<Arc<check::Compiled>>>,
}

impl PartialEq for Formula {
    /// Two formulas are equal when their expressions are (what is cached is not compared).
    fn eq(&self, other: &Self) -> bool {
        self.ast == other.ast
    }
}

impl std::fmt::Debug for Formula {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Formula")
            .field("ast", &self.ast)
            .finish_non_exhaustive()
    }
}

impl FormulaError {
    pub(crate) fn new(
        code: FormulaErrorCode,
        message: String,
        start: usize,
        end: usize,
        detail: String,
    ) -> Self {
        Self {
            code,
            message,
            start,
            end,
            detail,
        }
    }
}

/// Parses an expression (stored or display form: the grammar is the same).
///
/// # Errors
/// `Syntax` or `TooLong`.
pub fn parse(text: &str) -> Result<Formula, FormulaError> {
    parser::parse(text).map(|ast| Formula {
        ast,
        compiled: OnceLock::new(),
    })
}

/// Checks a parsed stored expression against the schema; `self_id` is the formula's own property id (a reference
/// to it, directly or through other formulas, is a `Cycle`).
///
/// # Errors
/// Any code but `Syntax` and `TooLong`.
pub fn check(
    formula: &Formula,
    host: &dyn FormulaSchema,
    self_id: Option<&str>,
) -> Result<FormulaType, FormulaError> {
    check::compile(&formula.ast, host, self_id).map(|compiled| compiled.root.ty.result_type())
}

/// Evaluates a parsed stored expression on a page; `None` when the result is empty (section 4.4), or when the
/// expression does not check against the host's schema.
#[must_use]
pub fn evaluate(
    formula: &Formula,
    page: PageId,
    host: &dyn FormulaHost,
    env: &FormulaEnv,
) -> Option<FormulaResult> {
    let compiled = formula
        .compiled
        .get_or_init(|| check::compile(&formula.ast, host, None).ok().map(Arc::new))
        .as_ref()?;
    eval::run(compiled, page, host, env)
}

/// Rewrites the names of `prop("…")` and `.prop("…")` to ids (`names` maps a name to an id, the title's names
/// included).
///
/// # Errors
/// `UnknownProperty` at the first name that maps to nothing.
pub fn to_stored(
    display: &str,
    names: &dyn Fn(&str) -> Option<String>,
) -> Result<String, FormulaError> {
    convert::to_stored(display, names)
}

/// Rewrites the ids of `prop("…")` and `.prop("…")` to names (an id `name_of` does not know stays as it is).
#[must_use]
pub fn to_display(stored: &str, name_of: &dyn Fn(&str) -> Option<String>) -> String {
    convert::to_display(stored, name_of)
}

/// Rewrites the ids of `prop("…")` and `.prop("…")` through `map` (an id it does not map stays as it is).
#[must_use]
pub fn remap_ids(stored: &str, map: &dyn Fn(&str) -> Option<String>) -> String {
    convert::remap_ids(stored, map)
}

/// The ids the `prop("…")` and `.prop("…")` calls of a stored expression name, in order, without repeats.
#[must_use]
pub fn referenced_ids(stored: &str) -> Vec<String> {
    convert::referenced_ids(stored)
}
