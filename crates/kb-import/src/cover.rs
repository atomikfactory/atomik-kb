//! Page covers as a source gives them (ADR-0040): the named forms atomik-kb writes in front matter
//! (`color:<palette name>`, `gradient:<name>`) and the position. A cover that is an image is a file
//! of the source and is resolved by the reader of the source (front matter: a relative path;
//! AppFlowy: a `local_files` name or an `https` URL).

/// The nine palette names a `color:` cover takes (the icon colours of `kb-core`'s `validate.rs`).
const COLORS: [&str; 9] = [
    "gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red",
];

/// The longest name of a gradient, in characters.
const GRADIENT_NAME_MAX: usize = 32;

/// A cover read from the source, as the page will have it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedCover {
    /// `color:<palette name>` or `gradient:<name>`, already in the grammar of the core.
    Named(String),
    /// A file of the source, uploaded by the shell and used as the cover when it is an image: a
    /// path of [`crate::Plan::needed_files`] or a key of [`crate::Plan::remote_files`].
    File(String),
}

/// `text` when it is a named cover in the grammar of the core: `color:` and one of the nine palette
/// names, or `gradient:` and 1 to 32 characters of `[a-z0-9-]`. `None` for anything else.
pub(crate) fn named(text: &str) -> Option<String> {
    let text = text.trim();
    if let Some(name) = text.strip_prefix("color:") {
        return COLORS.contains(&name).then(|| text.to_owned());
    }
    let name = text.strip_prefix("gradient:")?;
    let valid = !name.is_empty()
        && name.len() <= GRADIENT_NAME_MAX
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    valid.then(|| text.to_owned())
}

/// The vertical position a front matter value gives: an integer from 0 to 100 (written as text).
pub(crate) fn position(text: &str) -> Option<u8> {
    let y: u8 = text.trim().parse().ok()?;
    (y <= 100).then_some(y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_covers_follow_the_grammar_of_the_core() {
        for ok in [
            "color:blue",
            "color:gray",
            " color:red ",
            "gradient:sunset",
            "gradient:a",
            "gradient:blue-2",
        ] {
            assert_eq!(named(ok).as_deref(), Some(ok.trim()), "{ok}");
        }
        let long = format!("gradient:{}", "x".repeat(32));
        assert_eq!(named(&long).as_deref(), Some(long.as_str()));
        for bad in [
            "",
            "blue",
            "color:",
            "color:teal",
            "color:Blue",
            "gradient:",
            "gradient:Sun",
            "gradient:a_b",
            "gradient:a b",
            "attachment:5f0c1d2e-3a4b-4c5d-8e6f-0a1b2c3d4e5f",
            "images/banner.png",
            &format!("gradient:{}", "x".repeat(33)),
        ] {
            assert_eq!(named(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_position_is_an_integer_from_zero_to_one_hundred() {
        assert_eq!(position("0"), Some(0));
        assert_eq!(position(" 30 "), Some(30));
        assert_eq!(position("100"), Some(100));
        for bad in ["", "101", "-1", "30.5", "high", "255", "256", "1e2"] {
            assert_eq!(position(bad), None, "{bad}");
        }
    }
}
