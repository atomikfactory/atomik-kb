//! Names and ids (`formulas.md` section 6): the display form of an expression names properties
//! (`prop("Price")`), the stored form writes their ids. The four conversions work on tokens, never
//! on the syntax tree, so they apply to a text that does not fully parse; only the string argument
//! of `prop("…")` and `.prop("…")` is rewritten, every other byte is kept.

use super::lexer::{Tok, tokenize};
use super::{FormulaError, FormulaErrorCode};

/// The string argument of a `prop("…")` call: where it is and what it says.
struct PropArg {
    start: usize,
    end: usize,
    byte_start: usize,
    byte_end: usize,
    value: String,
}

/// The `prop` `(` string `)` sequences of a text, in order (the method form `.prop("…")` has the
/// same four tokens after the dot).
fn prop_args(text: &str) -> Vec<PropArg> {
    let tokens = tokenize(text);
    let mut found = Vec::new();
    for window in tokens.windows(4) {
        let [name, open, literal, close] = window else {
            continue;
        };
        if let (Tok::Ident(name), Tok::LParen, Tok::Str(value), Tok::RParen) =
            (&name.tok, &open.tok, &literal.tok, &close.tok)
            && name == "prop"
        {
            found.push(PropArg {
                start: literal.start,
                end: literal.end,
                byte_start: literal.byte_start,
                byte_end: literal.byte_end,
                value: value.clone(),
            });
        }
    }
    found
}

/// A string literal: `"` and `\` escaped, nothing else.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Rewrites the argument of every `prop` call the closure gives a new text for.
fn rewrite(
    text: &str,
    mut replace: impl FnMut(&PropArg) -> Result<Option<String>, FormulaError>,
) -> Result<String, FormulaError> {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for arg in prop_args(text) {
        if let Some(new) = replace(&arg)? {
            out.push_str(text.get(last..arg.byte_start).unwrap_or_default());
            out.push_str(&quote(&new));
            last = arg.byte_end;
        }
    }
    out.push_str(text.get(last..).unwrap_or_default());
    Ok(out)
}

pub(super) fn to_stored(
    display: &str,
    names: &dyn Fn(&str) -> Option<String>,
) -> Result<String, FormulaError> {
    rewrite(display, |arg| match names(&arg.value) {
        Some(id) => Ok(Some(id)),
        None => Err(FormulaError::new(
            FormulaErrorCode::UnknownProperty,
            format!("no property named \"{}\"", arg.value),
            arg.start,
            arg.end,
            arg.value.clone(),
        )),
    })
}

pub(super) fn to_display(stored: &str, name_of: &dyn Fn(&str) -> Option<String>) -> String {
    let rewritten = rewrite(stored, |arg| Ok(name_of(&arg.value)));
    // The closure never fails.
    rewritten.unwrap_or_else(|_| stored.to_owned())
}

pub(super) fn remap_ids(stored: &str, map: &dyn Fn(&str) -> Option<String>) -> String {
    let rewritten = rewrite(stored, |arg| Ok(map(&arg.value)));
    rewritten.unwrap_or_else(|_| stored.to_owned())
}

pub(super) fn referenced_ids(stored: &str) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for arg in prop_args(stored) {
        if !ids.contains(&arg.value) {
            ids.push(arg.value);
        }
    }
    ids
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn names(name: &str) -> Option<String> {
        match name {
            "Price" => Some("id-price".into()),
            "Qty \"x\"" => Some("id-qty".into()),
            "Name" | "title" => Some("title".into()),
            _ => None,
        }
    }

    fn name_of(id: &str) -> Option<String> {
        match id {
            "id-price" => Some("Price".into()),
            "id-qty" => Some("Qty \"x\" \\ 1".into()),
            "title" => Some("Name".into()),
            _ => None,
        }
    }

    #[test]
    fn display_to_stored_rewrites_only_the_arguments() {
        let display = "prop(\"Price\")  *  /* prop(\"Nope\") */ \"Price\" + x.prop(\"Name\")\n";
        assert_eq!(
            to_stored(display, &names).unwrap(),
            "prop(\"id-price\")  *  /* prop(\"Nope\") */ \"Price\" + x.prop(\"title\")\n"
        );
        // names with quotes are written escaped on the way back, matched on the way in
        assert_eq!(
            to_stored("prop(\"Qty \\\"x\\\"\")", &names).unwrap(),
            "prop(\"id-qty\")"
        );
    }

    #[test]
    fn an_unknown_name_is_refused_at_its_argument() {
        let error = to_stored("1 + prop(\"Nope\")", &names).unwrap_err();
        assert_eq!(error.code, FormulaErrorCode::UnknownProperty);
        assert_eq!((error.start, error.end), (9, 15));
        assert_eq!(error.detail, "Nope");
        // positions are characters
        let error = to_stored("\"é😀\" + prop(\"Nope\")", &names).unwrap_err();
        assert_eq!(error.start, 12);
    }

    #[test]
    fn stored_to_display_escapes_names() {
        let stored = "prop(\"id-qty\") + prop(\"gone\") + prop(\"title\")";
        assert_eq!(
            to_display(stored, &name_of),
            "prop(\"Qty \\\"x\\\" \\\\ 1\") + prop(\"gone\") + prop(\"Name\")"
        );
    }

    #[test]
    fn remap_changes_mapped_ids_only() {
        let map = |id: &str| (id == "a").then(|| "b".to_owned());
        assert_eq!(
            remap_ids("prop(\"a\") + prop(\"c\") + .prop(\"a\"", &map),
            "prop(\"b\") + prop(\"c\") + .prop(\"a\""
        );
        assert_eq!(remap_ids("x.prop(\"a\")", &map), "x.prop(\"b\")");
    }

    #[test]
    fn conversions_work_on_text_that_does_not_parse() {
        let broken = "prop(\"Price\") + (  prop(\"Name\") * \"unclosed";
        assert_eq!(
            to_stored(broken, &names).unwrap(),
            "prop(\"id-price\") + (  prop(\"title\") * \"unclosed"
        );
        let lexical = "prop(\"Price\") # prop(\"Name\") /* open";
        assert_eq!(
            to_stored(lexical, &names).unwrap(),
            "prop(\"id-price\") # prop(\"title\") /* open"
        );
        assert_eq!(
            referenced_ids("prop(\"a\") + prop(\"b\") + prop(\"a\") + prop(\"c\""),
            ["a", "b"]
        );
    }

    #[test]
    fn only_a_string_literal_in_a_prop_call_counts() {
        let map = |_: &str| Some("z".to_owned());
        assert_eq!(
            remap_ids(
                "prop(x) + prop(\"a\", 1) + props(\"a\") + prop (\"a\")",
                &map
            ),
            "prop(x) + prop(\"a\", 1) + props(\"a\") + prop (\"z\")"
        );
        assert_eq!(referenced_ids("prop(\"a\") + x.prop(\"b\")"), ["a", "b"]);
        assert!(referenced_ids("1 + 2").is_empty());
    }
}
