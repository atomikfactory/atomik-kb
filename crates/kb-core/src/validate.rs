//! Input validation shared by request handlers. Every failure is an
//! `ApiError::invalid(field, ...)` so shells can highlight the offending field.

use kb_types::{ApiError, Json};

/// Trims `raw`; requires 1..=`max_chars` characters and no control characters.
pub fn display_name(raw: &str, max_chars: usize, field: &str) -> Result<String, ApiError> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(ApiError::invalid(field, "must not be empty"));
    }
    bounded_text(name, max_chars, field)
}

/// Trims `raw`; allows empty; requires at most `max_chars` characters and no
/// control characters (so no newlines either).
pub fn optional_text(raw: &str, max_chars: usize, field: &str) -> Result<String, ApiError> {
    bounded_text(raw.trim(), max_chars, field)
}

/// The longest page title, in characters.
pub const MAX_PAGE_TITLE_CHARS: usize = 500;

/// Page titles: 0 to [`MAX_PAGE_TITLE_CHARS`] characters.
pub fn page_title(raw: &str) -> Result<String, ApiError> {
    optional_text(raw, MAX_PAGE_TITLE_CHARS, "title")
}

/// Search queries: trimmed, 1 to 200 characters, no control characters.
pub fn search_query(raw: &str) -> Result<String, ApiError> {
    display_name(raw, 200, "query")
}

/// Search limits are clamped to 1..=100.
pub fn search_limit(limit: u32) -> u32 {
    limit.clamp(1, kb_storage::MAX_SEARCH_LIMIT)
}

/// The prefix of an icon from the bundled set: `icon:<name>` or `icon:<name>:<color>` (ADR-0029).
const ICON_SET_PREFIX: &str = "icon:";
/// The prefix of an uploaded image icon: `attachment:<attachment id>` (pages only).
pub const ICON_ATTACHMENT_PREFIX: &str = "attachment:";
/// The longest name of an icon of the set, in characters.
const ICON_NAME_MAX: usize = 48;
/// The nine colours of an icon of the set (absent: the text colour).
const ICON_COLORS: [&str; 9] = [
    "gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red",
];

/// Icons (ADR-0029): `None` clears; `Some` is trimmed and is one of
///
/// - an emoji or short text: 1 to 32 bytes, no control characters, not starting with `icon:` or
///   `attachment:`;
/// - an icon of the bundled set: `icon:<name>` or `icon:<name>:<color>`, the name 1 to 48 characters of
///   `[a-z0-9-]` not starting or ending with `-`, the colour one of the nine of the palette;
/// - an uploaded image: `attachment:<attachment id>`, the id a lowercase hyphenated UUID.
///
/// This is the grammar only. Whether an attachment exists, belongs to the page's space and is an
/// image is checked by the page handler, and a space refuses the third form ([`space_icon`]).
pub fn icon(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let icon = raw.trim();
    if icon.is_empty() {
        return Err(ApiError::invalid(
            "icon",
            "must not be empty; omit it to clear",
        ));
    }
    if let Some(rest) = icon.strip_prefix(ICON_SET_PREFIX) {
        icon_of_the_set(rest)?;
    } else if let Some(rest) = icon.strip_prefix(ICON_ATTACHMENT_PREFIX) {
        icon_attachment_id(rest)?;
    } else {
        if icon.len() > 32 {
            return Err(ApiError::invalid("icon", "must be at most 32 bytes"));
        }
        if icon.chars().any(char::is_control) {
            return Err(ApiError::invalid(
                "icon",
                "must not contain control characters",
            ));
        }
    }
    Ok(Some(icon.to_owned()))
}

/// Icons of a space: the same grammar without the uploaded image (a space icon cannot be an image).
pub fn space_icon(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let icon = icon(raw)?;
    if icon
        .as_deref()
        .is_some_and(|icon| icon.starts_with(ICON_ATTACHMENT_PREFIX))
    {
        return Err(ApiError::invalid("icon", "a space icon cannot be an image"));
    }
    Ok(icon)
}

/// The attachment an icon names (`attachment:<id>`), `None` for every other form and for an id
/// that is not a canonical UUID. Reads what [`icon`] accepted; never fails.
#[must_use]
pub fn icon_attachment(icon: &str) -> Option<kb_types::AttachmentId> {
    let id = icon.strip_prefix(ICON_ATTACHMENT_PREFIX)?;
    icon_attachment_id(id).ok()
}

/// `<name>` or `<name>:<color>` after `icon:`.
fn icon_of_the_set(rest: &str) -> Result<(), ApiError> {
    let (name, color) = match rest.split_once(':') {
        Some((name, color)) => (name, Some(color)),
        None => (rest, None),
    };
    if name.is_empty() {
        return Err(ApiError::invalid(
            "icon",
            "an icon of the set is icon:<name> or icon:<name>:<color>; the name is missing",
        ));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(ApiError::invalid(
            "icon",
            "the icon name may only contain a-z, 0-9 and '-'",
        ));
    }
    if name.len() > ICON_NAME_MAX {
        return Err(ApiError::invalid(
            "icon",
            format!("the icon name must be at most {ICON_NAME_MAX} characters"),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(ApiError::invalid(
            "icon",
            "the icon name must not start or end with '-'",
        ));
    }
    if let Some(color) = color
        && !ICON_COLORS.contains(&color)
    {
        return Err(ApiError::invalid(
            "icon",
            format!(
                "the icon colour must be one of: {}; omit it for the text colour",
                ICON_COLORS.join(", ")
            ),
        ));
    }
    Ok(())
}

/// The id after `attachment:`: a lowercase hyphenated UUID (8-4-4-4-12 hex digits).
fn icon_attachment_id(text: &str) -> Result<kb_types::AttachmentId, ApiError> {
    attachment_id_of("icon", text)
}

/// [`icon_attachment_id`] reporting a refusal on `field`.
fn attachment_id_of(field: &str, text: &str) -> Result<kb_types::AttachmentId, ApiError> {
    let canonical = text.len() == 36
        && text.bytes().enumerate().all(|(at, b)| match at {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        });
    if !canonical {
        return Err(ApiError::invalid(
            field,
            "the attachment id must be a lowercase hyphenated UUID",
        ));
    }
    text.parse().map_err(|_| {
        ApiError::invalid(
            field,
            "the attachment id must be a lowercase hyphenated UUID",
        )
    })
}

/// The prefix of a cover that is a colour of the palette: `color:<palette name>` (ADR-0040).
const COVER_COLOR_PREFIX: &str = "color:";
/// The prefix of a cover that is a bundled gradient: `gradient:<name>`. The UI defines the list;
/// the core checks the shape of the name only, so a newer client's gradient syncs to an older one.
const COVER_GRADIENT_PREFIX: &str = "gradient:";
/// The longest name of a gradient, in characters.
const COVER_GRADIENT_NAME_MAX: usize = 32;
/// The largest vertical position of a cover, in percent.
const COVER_Y_MAX: u8 = kb_model::COVER_Y_MAX;

/// Covers (ADR-0040): `None` clears; `Some` is trimmed and is one of
///
/// - an uploaded image: `attachment:<attachment id>`, the id a lowercase hyphenated UUID;
/// - a colour of the palette: `color:<name>`, the name one of the nine of the icon colours;
/// - a bundled gradient: `gradient:<name>`, the name 1 to 32 characters of `[a-z0-9-]`.
///
/// This is the grammar only. Whether an attachment exists, belongs to the page's space and is an
/// image is checked by the page handler.
pub fn cover(raw: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let cover = raw.trim();
    if cover.is_empty() {
        return Err(ApiError::invalid(
            "cover",
            "must not be empty; omit it to clear",
        ));
    }
    if let Some(rest) = cover.strip_prefix(ICON_ATTACHMENT_PREFIX) {
        attachment_id_of("cover", rest)?;
    } else if let Some(name) = cover.strip_prefix(COVER_COLOR_PREFIX) {
        if !ICON_COLORS.contains(&name) {
            return Err(ApiError::invalid(
                "cover",
                format!(
                    "the cover colour must be one of: {}",
                    ICON_COLORS.join(", ")
                ),
            ));
        }
    } else if let Some(name) = cover.strip_prefix(COVER_GRADIENT_PREFIX) {
        if name.is_empty() {
            return Err(ApiError::invalid(
                "cover",
                "a gradient cover is gradient:<name>; the name is missing",
            ));
        }
        if !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(ApiError::invalid(
                "cover",
                "the gradient name may only contain a-z, 0-9 and '-'",
            ));
        }
        if name.len() > COVER_GRADIENT_NAME_MAX {
            return Err(ApiError::invalid(
                "cover",
                format!("the gradient name must be at most {COVER_GRADIENT_NAME_MAX} characters"),
            ));
        }
    } else {
        return Err(ApiError::invalid(
            "cover",
            "a cover is attachment:<id>, color:<name> or gradient:<name>",
        ));
    }
    Ok(Some(cover.to_owned()))
}

/// The attachment a cover names (`attachment:<id>`), `None` for every other form and for an id that
/// is not a canonical UUID. Reads what [`cover`] accepted; never fails.
#[must_use]
pub fn cover_attachment(cover: &str) -> Option<kb_types::AttachmentId> {
    let id = cover.strip_prefix(ICON_ATTACHMENT_PREFIX)?;
    attachment_id_of("cover", id).ok()
}

/// The vertical position of a cover: `None` (the default) or 0 to 100 percent.
pub fn cover_y(raw: Option<u8>) -> Result<Option<u8>, ApiError> {
    match raw {
        Some(y) if y > COVER_Y_MAX => Err(ApiError::invalid(
            "coverY",
            format!("the cover position must be between 0 and {COVER_Y_MAX}"),
        )),
        other => Ok(other),
    }
}

fn bounded_text(text: &str, max_chars: usize, field: &str) -> Result<String, ApiError> {
    if text.chars().count() > max_chars {
        return Err(ApiError::invalid(
            field,
            format!("must be at most {max_chars} characters"),
        ));
    }
    if text.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            field,
            "must not contain control characters",
        ));
    }
    Ok(text.to_owned())
}

/// Setting keys look like `namespace.name[.more]`, lowercase `[a-z0-9_.-]`,
/// namespaces `ui` or `app`, at most 128 bytes, no empty segments.
pub fn setting_key(key: &str) -> Result<(), ApiError> {
    const MAX_LEN: usize = 128;
    const NAMESPACES: [&str; 2] = ["ui", "app"];

    if key.is_empty() || key.len() > MAX_LEN {
        return Err(ApiError::invalid(
            "key",
            format!("must be 1 to {MAX_LEN} bytes"),
        ));
    }
    if !key
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-'))
    {
        return Err(ApiError::invalid(
            "key",
            "may only contain a-z, 0-9, '_', '-' and '.'",
        ));
    }
    let mut segments = key.split('.');
    let namespace = segments.next().unwrap_or_default();
    if !NAMESPACES.contains(&namespace) {
        return Err(ApiError::invalid(
            "key",
            format!("must start with one of: {}", NAMESPACES.join(", ")),
        ));
    }
    let mut rest = segments.peekable();
    if rest.peek().is_none() {
        return Err(ApiError::invalid(
            "key",
            "must have a name after the namespace",
        ));
    }
    if rest.any(str::is_empty) {
        return Err(ApiError::invalid("key", "must not contain empty segments"));
    }
    Ok(())
}

/// Rejects values whose JSON serialisation exceeds `max_bytes`.
pub fn setting_value(value: &Json, max_bytes: usize) -> Result<(), ApiError> {
    let size = serde_json::to_vec(value)
        .map_err(|e| ApiError::invalid("value", format!("not serialisable: {e}")))?
        .len();
    if size > max_bytes {
        return Err(ApiError::invalid(
            "value",
            format!("serialised value must be at most {max_bytes} bytes"),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(display_name("  a  ", 10, "n").unwrap(), "a");
        assert!(display_name("", 10, "n").is_err());
        assert!(display_name("abcdefghijk", 10, "n").is_err());
        assert!(display_name("a\tb", 10, "n").is_err());
        assert_eq!(display_name("héllo wörld", 11, "n").unwrap(), "héllo wörld");
    }

    #[test]
    fn titles_may_be_empty_but_not_multiline() {
        assert_eq!(page_title("  ").unwrap(), "");
        assert_eq!(page_title(" Plan ").unwrap(), "Plan");
        assert!(page_title("a\nb").is_err());
        assert!(page_title(&"x".repeat(501)).is_err());
        assert_eq!(page_title(&"x".repeat(500)).unwrap().len(), 500);
    }

    #[test]
    fn icons_clear_with_none_and_reject_empty() {
        assert_eq!(icon(None).unwrap(), None);
        assert_eq!(icon(Some(" 📚 ")).unwrap(), Some("📚".to_owned()));
        assert!(icon(Some("  ")).is_err());
        assert!(icon(Some(&"a".repeat(33))).is_err());
        assert!(icon(Some("a\u{7}")).is_err());
    }

    fn field_of(err: &ApiError) -> Option<&str> {
        err.details.as_ref()?.get("field")?.as_str()
    }

    /// The message of a refused icon (and that it is on the field `icon`).
    fn refusal(raw: &str) -> String {
        let err = icon(Some(raw)).unwrap_err();
        assert_eq!(field_of(&err), Some("icon"), "{raw}: {err}");
        err.message
    }

    const UUID: &str = "5f0c1d2e-3a4b-4c5d-8e6f-0a1b2c3d4e5f";

    #[test]
    fn icons_accept_emoji_and_short_text_up_to_32_bytes() {
        for ok in ["🌱", "A", "ab", "👨‍👩‍👧‍👦", "Notes", "icons:book", "attachments"]
        {
            assert_eq!(icon(Some(ok)).unwrap().as_deref(), Some(ok), "{ok}");
        }
        assert_eq!(
            icon(Some(&"a".repeat(32))).unwrap().map(|s| s.len()),
            Some(32)
        );
        assert!(refusal(&"a".repeat(33)).contains("32 bytes"));
        // The prefixes are case-sensitive: this is plain text.
        assert!(icon(Some("Icon:book")).is_ok());
        assert!(icon(Some("ATTACHMENT:x")).is_ok());
        assert!(refusal("a\nb").contains("control"));
        assert!(refusal("\u{7f}x").contains("control"));
    }

    #[test]
    fn icons_of_the_set_follow_the_grammar_rule_by_rule() {
        for ok in [
            "icon:a",
            "icon:book-open",
            "icon:book-open:blue",
            "icon:0",
            "icon:a-b-c-9:gray",
            &format!("icon:{}", "x".repeat(48)),
            &format!("icon:{}:purple", "x".repeat(48)),
        ] {
            assert_eq!(icon(Some(ok)).unwrap().as_deref(), Some(ok), "{ok}");
        }
        for colour in [
            "gray", "brown", "orange", "yellow", "green", "blue", "purple", "pink", "red",
        ] {
            assert!(
                icon(Some(&format!("icon:star:{colour}"))).is_ok(),
                "{colour}"
            );
        }
        // Trimmed like every icon.
        assert_eq!(
            icon(Some("  icon:book-open:blue ")).unwrap().as_deref(),
            Some("icon:book-open:blue")
        );

        // Each failing rule has its own message.
        let missing = refusal("icon:");
        assert!(missing.contains("name is missing"), "{missing}");
        assert!(refusal("icon::blue").contains("name is missing"));
        let characters = refusal("icon:Book");
        assert!(characters.contains("a-z, 0-9"), "{characters}");
        assert!(refusal("icon:book open").contains("a-z, 0-9"));
        assert!(refusal("icon:book_open").contains("a-z, 0-9"));
        assert!(refusal("icon:livre\u{e9}").contains("a-z, 0-9"));
        let long = refusal(&format!("icon:{}", "x".repeat(49)));
        assert!(long.contains("at most 48"), "{long}");
        let dash = refusal("icon:-book");
        assert!(dash.contains("start or end"), "{dash}");
        assert!(refusal("icon:book-").contains("start or end"));
        assert!(refusal("icon:-").contains("start or end"));
        let colour = refusal("icon:book:teal");
        assert!(colour.contains("colour must be one of"), "{colour}");
        assert!(
            colour.contains("gray") && colour.contains("red"),
            "{colour}"
        );
        assert!(refusal("icon:book:").contains("colour must be one of"));
        assert!(refusal("icon:book:Blue").contains("colour must be one of"));
        assert!(refusal("icon:book:blue:red").contains("colour must be one of"));
        let all = [missing, characters, long, dash, colour];
        for (n, a) in all.iter().enumerate() {
            for b in &all[n + 1..] {
                assert_ne!(a, b, "the rules are told apart");
            }
        }
    }

    #[test]
    fn image_icons_need_a_lowercase_hyphenated_uuid() {
        let ok = format!("attachment:{UUID}");
        assert_eq!(icon(Some(&ok)).unwrap().as_deref(), Some(ok.as_str()));
        assert_eq!(
            icon(Some(&format!(" {ok} "))).unwrap().as_deref(),
            Some(ok.as_str())
        );
        assert_eq!(
            icon_attachment(&ok).map(|id| id.to_string()),
            Some(UUID.to_owned())
        );
        for bad in [
            "attachment:",
            "attachment:abc",
            &format!("attachment:{}", UUID.to_uppercase()),
            &format!("attachment:{}", UUID.replace('-', "")),
            &format!("attachment:{UUID}0"),
            &format!("attachment:{}", &UUID[1..]),
            &format!("attachment:{}g", &UUID[..35]),
            &format!("attachment:{{{UUID}}}"),
            &format!("attachment: {UUID}"),
            &format!("attachment:{UUID}:blue"),
        ] {
            let message = refusal(bad);
            assert!(
                message.contains("lowercase hyphenated UUID"),
                "{bad}: {message}"
            );
        }
        // The other forms name no attachment.
        assert_eq!(icon_attachment("🌱"), None);
        assert_eq!(icon_attachment("icon:book"), None);
        assert_eq!(icon_attachment("attachment:nope"), None);
    }

    /// The message of a refused cover (and that it is on the field `cover`).
    fn cover_refusal(raw: &str) -> String {
        let err = cover(Some(raw)).unwrap_err();
        assert_eq!(field_of(&err), Some("cover"), "{raw}: {err}");
        err.message
    }

    #[test]
    fn covers_are_an_image_a_palette_colour_or_a_named_gradient() {
        assert_eq!(cover(None).unwrap(), None);
        let image = format!("attachment:{UUID}");
        for ok in [
            image.as_str(),
            "color:gray",
            "color:blue",
            "color:red",
            "gradient:sunset",
            "gradient:a",
            "gradient:blue-2-pink",
            &format!("gradient:{}", "x".repeat(32)),
        ] {
            assert_eq!(cover(Some(ok)).unwrap().as_deref(), Some(ok), "{ok}");
        }
        for colour in ICON_COLORS {
            assert!(cover(Some(&format!("color:{colour}"))).is_ok(), "{colour}");
        }
        // Trimmed like an icon.
        assert_eq!(
            cover(Some(" gradient:dawn ")).unwrap().as_deref(),
            Some("gradient:dawn")
        );
        assert_eq!(
            cover_attachment(&image).map(|id| id.to_string()),
            Some(UUID.to_owned())
        );
        assert_eq!(cover_attachment("color:blue"), None);
        assert_eq!(cover_attachment("gradient:dawn"), None);
        assert_eq!(cover_attachment("attachment:nope"), None);
    }

    #[test]
    fn each_failing_cover_rule_has_its_own_message_on_the_field_cover() {
        assert!(cover_refusal("").contains("must not be empty"));
        assert!(cover_refusal("   ").contains("must not be empty"));
        // Not a form of cover: an emoji, an icon, a plain word, a URL.
        for bad in [
            "🌱",
            "icon:book",
            "blue",
            "https://example.com/a.png",
            "COLOR:blue",
        ] {
            assert!(
                cover_refusal(bad).contains("attachment:<id>, color:<name> or gradient:<name>"),
                "{bad}"
            );
        }
        for bad in [
            "attachment:",
            "attachment:abc",
            &format!("attachment:{}", UUID.to_uppercase()),
            &format!("attachment:{}", UUID.replace('-', "")),
            &format!("attachment:{UUID}0"),
            &format!("attachment: {UUID}"),
        ] {
            assert!(
                cover_refusal(bad).contains("lowercase hyphenated UUID"),
                "{bad}"
            );
        }
        let colour = cover_refusal("color:teal");
        assert!(colour.contains("cover colour must be one of"), "{colour}");
        assert!(
            colour.contains("gray") && colour.contains("red"),
            "{colour}"
        );
        assert!(cover_refusal("color:").contains("cover colour must be one of"));
        assert!(cover_refusal("color:Blue").contains("cover colour must be one of"));
        let missing = cover_refusal("gradient:");
        assert!(missing.contains("name is missing"), "{missing}");
        let characters = cover_refusal("gradient:Sunset");
        assert!(characters.contains("a-z, 0-9"), "{characters}");
        assert!(cover_refusal("gradient:sun set").contains("a-z, 0-9"));
        assert!(cover_refusal("gradient:sun_set").contains("a-z, 0-9"));
        assert!(cover_refusal("gradient:blue:red").contains("a-z, 0-9"));
        let long = cover_refusal(&format!("gradient:{}", "x".repeat(33)));
        assert!(long.contains("at most 32"), "{long}");
        let all = [missing, characters, long, colour];
        for (n, a) in all.iter().enumerate() {
            for b in &all[n + 1..] {
                assert_ne!(a, b, "the rules are told apart");
            }
        }
    }

    #[test]
    fn a_cover_position_is_none_or_zero_to_one_hundred() {
        for ok in [None, Some(0), Some(50), Some(100)] {
            assert_eq!(cover_y(ok).unwrap(), ok);
        }
        for bad in [101u8, 150, 255] {
            let err = cover_y(Some(bad)).unwrap_err();
            assert_eq!(field_of(&err), Some("coverY"), "{bad}");
        }
    }

    #[test]
    fn a_space_icon_refuses_an_image_and_nothing_else_changes() {
        assert_eq!(space_icon(None).unwrap(), None);
        assert_eq!(space_icon(Some(" 💼 ")).unwrap().as_deref(), Some("💼"));
        assert_eq!(
            space_icon(Some("icon:briefcase:brown")).unwrap().as_deref(),
            Some("icon:briefcase:brown")
        );
        let err = space_icon(Some(&format!("attachment:{UUID}"))).unwrap_err();
        assert_eq!(field_of(&err), Some("icon"));
        assert_eq!(err.message, "a space icon cannot be an image");
        // A malformed image icon is told apart from a well-formed one the space may not have.
        let err = space_icon(Some("attachment:x")).unwrap_err();
        assert!(err.message.contains("UUID"), "{err}");
        assert!(space_icon(Some("")).is_err());
        assert!(space_icon(Some(&"a".repeat(33))).is_err());
    }

    #[test]
    fn keys_follow_the_grammar() {
        for ok in ["ui.theme", "app.last-dir", "ui.editor.font_size", "app.x1"] {
            assert!(setting_key(ok).is_ok(), "{ok}");
        }
        for bad in [
            "", "ui", "ui.", ".ui", "ui..a", "Ui.theme", "sys.x", "ui.a b", "ui.é",
        ] {
            assert!(setting_key(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn search_inputs_are_bounded() {
        assert_eq!(search_query("  notes ").unwrap(), "notes");
        assert!(search_query(" ").is_err());
        assert!(search_query(&"q".repeat(201)).is_err());
        assert_eq!(search_limit(0), 1);
        assert_eq!(search_limit(20), 20);
        assert_eq!(search_limit(1_000), 100);
    }

    #[test]
    fn values_are_size_limited() {
        assert!(setting_value(&serde_json::json!({"a": 1}), 64).is_ok());
        assert!(setting_value(&Json::String("x".repeat(100)), 64).is_err());
    }
}
