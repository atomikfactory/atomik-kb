//! A small reader of HTML tags, for the HTML that sits inside Markdown: inline tags (`<u>`,
//! `<time datetime>`), and the tags that open and close containers across blocks (`<details>`,
//! `<summary>`, `<aside>`) and the annotation comments of atomik-kb's own exports. Whole HTML
//! documents and blocks go through `scraper` instead (`crate::html`).

/// The longest tag read, in bytes. Every scan of a tag stays inside this window, so a flood of
/// broken tags costs a bounded amount per tag.
const MAX_TAG_BYTES: usize = 1_000;

/// One tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Tag {
    /// Lowercase name.
    pub name: String,
    /// `</name>`.
    pub closing: bool,
    /// `<name ... />`.
    pub self_closing: bool,
    /// Attributes, names in lowercase, values with entities decoded.
    pub attrs: Vec<(String, String)>,
}

impl Tag {
    /// An attribute's value (`""` for an attribute without a value).
    pub(crate) fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Reads the tag at the start of `text` (which must start with `<`): the tag and the number of
/// bytes it takes. `None` for anything that is not a well-formed tag (a comment included).
pub(crate) fn parse_tag(text: &str) -> Option<(Tag, usize)> {
    let mut window = text.len().min(MAX_TAG_BYTES);
    while !text.is_char_boundary(window) {
        window -= 1;
    }
    let text = &text[..window];
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'<') {
        return None;
    }
    let mut at = 1;
    let closing = bytes.get(at) == Some(&b'/');
    if closing {
        at += 1;
    }
    let name_start = at;
    while bytes
        .get(at)
        .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'-')
    {
        at += 1;
    }
    if at == name_start || !bytes[name_start].is_ascii_alphabetic() {
        return None;
    }
    let name = text[name_start..at].to_ascii_lowercase();
    let mut attrs = Vec::new();
    loop {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        match bytes.get(at)? {
            b'>' => {
                return Some((
                    Tag {
                        name,
                        closing,
                        self_closing: false,
                        attrs,
                    },
                    at + 1,
                ));
            }
            b'/' => {
                if bytes.get(at + 1) == Some(&b'>') {
                    return Some((
                        Tag {
                            name,
                            closing,
                            self_closing: true,
                            attrs,
                        },
                        at + 2,
                    ));
                }
                at += 1;
            }
            _ => {
                let start = at;
                while bytes.get(at).is_some_and(|b| {
                    !b.is_ascii_whitespace()
                        && !matches!(*b, b'=' | b'>' | b'/' | b'"' | b'\'' | b'<')
                }) {
                    at += 1;
                }
                if at == start {
                    if bytes.get(at) == Some(&b'<') {
                        // A new tag starts: this one is broken.
                        return None;
                    }
                    // A stray quote or `=`: skip it.
                    at += 1;
                    continue;
                }
                let key = text[start..at].to_ascii_lowercase();
                while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                    at += 1;
                }
                let mut value = String::new();
                if bytes.get(at) == Some(&b'=') {
                    at += 1;
                    while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                        at += 1;
                    }
                    match bytes.get(at)? {
                        quote @ (b'"' | b'\'') => {
                            let quote = *quote;
                            let value_start = at + 1;
                            let end = bytes[value_start..].iter().position(|b| *b == quote)?;
                            value = decode_entities(&text[value_start..value_start + end]);
                            at = value_start + end + 1;
                        }
                        _ => {
                            let value_start = at;
                            while bytes
                                .get(at)
                                .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'>')
                            {
                                at += 1;
                            }
                            value = decode_entities(&text[value_start..at]);
                        }
                    }
                }
                attrs.push((key, value));
            }
        }
    }
}

/// Decodes the character references of attribute values: the five of XML, `&nbsp;` and numeric
/// ones; any other stays as written.
pub(crate) fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.find(';').filter(|end| *end <= 10).and_then(|end| {
            let name = &rest[1..end];
            let character = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => name
                    .strip_prefix('#')
                    .and_then(|number| match number.strip_prefix(['x', 'X']) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => number.parse::<u32>().ok(),
                    })
                    .and_then(char::from_u32),
            };
            character.map(|c| (c, end + 1))
        });
        match decoded {
            Some((character, used)) => {
                out.push(character);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A piece of an HTML block of Markdown.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Token<'a> {
    /// The JSON of an `<!-- atomik:... -->` comment.
    Annotation(&'a str),
    /// A `details`, `summary` or `aside` tag.
    Container(Tag),
    /// Anything else, as written.
    Raw(&'a str),
}

/// Whether a tag name opens or closes a container that spans blocks.
fn is_container(name: &str) -> bool {
    matches!(name, "details" | "summary" | "aside")
}

/// Splits the text of an HTML block into annotations, container tags and the rest. Comments that
/// are not annotations are dropped.
pub(crate) fn tokens(html: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut raw_start = 0;
    let mut at = 0;
    while let Some(found) = html[at..].find('<') {
        let start = at + found;
        let rest = &html[start..];
        if let Some(after) = rest.strip_prefix("<!--") {
            let (inner, used) = match after.find("-->") {
                Some(end) => (&after[..end], 4 + end + 3),
                None => (after, rest.len()),
            };
            push_raw(&mut out, &html[raw_start..start]);
            if let Some(json) = inner.trim().strip_prefix("atomik:") {
                out.push(Token::Annotation(json.trim()));
            }
            at = start + used;
            raw_start = at;
            continue;
        }
        match parse_tag(rest) {
            Some((tag, used)) if is_container(&tag.name) => {
                push_raw(&mut out, &html[raw_start..start]);
                out.push(Token::Container(tag));
                at = start + used;
                raw_start = at;
            }
            _ => at = start + 1,
        }
    }
    push_raw(&mut out, &html[raw_start..]);
    out
}

fn push_raw<'a>(out: &mut Vec<Token<'a>>, text: &'a str) {
    if !text.trim().is_empty() {
        out.push(Token::Raw(text));
    }
}

/// The text of a toggle's `<summary>` when it is one heading element, `<h1>` to `<h6>` (any case,
/// attributes allowed): the level, clamped to 1 to 3 as headings are, and what is inside the
/// element. `None` for anything else (text before or after the element, two headings, no
/// closing tag). Atomik-kb writes toggle headings this way (ADR-0040) and other tools do too.
pub(crate) fn summary_heading(summary: &str) -> Option<(u8, &str)> {
    let summary = summary.trim();
    let (tag, used) = parse_tag(summary)?;
    if tag.closing || tag.self_closing {
        return None;
    }
    let digit = match tag.name.as_bytes() {
        [b'h', digit @ b'1'..=b'6'] => digit - b'0',
        _ => return None,
    };
    let rest = summary.get(used..)?;
    let end = rest.len().checked_sub(5)?;
    let close = rest.as_bytes().get(end..)?;
    if !close.eq_ignore_ascii_case(format!("</h{digit}>").as_bytes()) {
        return None;
    }
    let inner = rest.get(..end)?;
    if inner.to_ascii_lowercase().contains("</h") {
        return None; // two headings, not one
    }
    Some((digit.clamp(1, 3), inner))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn reads_tags_and_attributes() {
        let (tag, used) =
            parse_tag(r#"<time datetime="2026-10-07T14:30" data-reminder=15>x"#).unwrap();
        assert_eq!(tag.name, "time");
        assert_eq!(tag.attr("datetime"), Some("2026-10-07T14:30"));
        assert_eq!(tag.attr("data-reminder"), Some("15"));
        assert!(!tag.closing && !tag.self_closing);
        assert_eq!(
            &r#"<time datetime="2026-10-07T14:30" data-reminder=15>x"#[used..],
            "x"
        );

        let (tag, _) = parse_tag("</U>").unwrap();
        assert!(tag.closing);
        assert_eq!(tag.name, "u");

        let (tag, _) = parse_tag("<br/>").unwrap();
        assert!(tag.self_closing);
        let (tag, _) = parse_tag("<details open>").unwrap();
        assert_eq!(tag.attr("open"), Some(""));
        let (tag, _) = parse_tag("<a href='x&amp;y' title=\"it's\">").unwrap();
        assert_eq!(tag.attr("href"), Some("x&y"));
        assert_eq!(tag.attr("title"), Some("it's"));
    }

    #[test]
    fn reads_the_heading_of_a_toggle_summary() {
        assert_eq!(summary_heading("<h2>Title</h2>"), Some((2, "Title")));
        assert_eq!(
            summary_heading("  <H1 id=\"x\">A &amp; B</H1>\n"),
            Some((1, "A &amp; B"))
        );
        assert_eq!(summary_heading("<h1></h1>"), Some((1, "")));
        assert_eq!(summary_heading("<h5>Deep</h5>"), Some((3, "Deep")));
        assert_eq!(
            summary_heading("<h3>a <b>b</b></h3>"),
            Some((3, "a <b>b</b>"))
        );
        for text in [
            "Title",
            "",
            "<h2>Title",
            "a <h2>b</h2>",
            "<h2>b</h2> a",
            "<h2>a</h2><h2>b</h2>",
            "<h2>a</h3>",
            "<h7>a</h7>",
            "<h2/>",
            "</h2>",
            "<p>a</p>",
        ] {
            assert_eq!(summary_heading(text), None, "{text:?}");
        }
    }

    #[test]
    fn refuses_what_is_not_a_tag() {
        for text in [
            "<",
            "< a>",
            "<3",
            "<!-- c -->",
            "<a",
            "<a href=\"x",
            "plain",
            "</>",
            "<1a>",
            "<a <b>",
        ] {
            assert!(parse_tag(text).is_none(), "{text:?}");
        }
        assert!(parse_tag(&format!("<a {}>", "x ".repeat(5_000))).is_none());
        assert!(parse_tag(&format!("<a x=\"{}\">", "y".repeat(5_000))).is_none());
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            decode_entities("a &amp; b &lt;c&gt; &quot;d&quot; &#39;e&#39; &#x41;&nbsp;"),
            "a & b <c> \"d\" 'e' A\u{a0}"
        );
        assert_eq!(
            decode_entities("&bogus; & &#xZZ; &#99999999;"),
            "&bogus; & &#xZZ; &#99999999;"
        );
        assert_eq!(decode_entities("no entities"), "no entities");
    }

    #[test]
    fn splits_html_blocks_into_tokens() {
        let toks = tokens("<details open>\n<summary>Title</summary>\n");
        assert!(
            matches!(&toks[0], Token::Container(t) if t.name == "details" && t.attr("open").is_some())
        );
        assert!(matches!(&toks[1], Token::Container(t) if t.name == "summary" && !t.closing));
        assert_eq!(toks[2], Token::Raw("Title"));
        assert!(matches!(&toks[3], Token::Container(t) if t.name == "summary" && t.closing));
        assert_eq!(toks.len(), 4);

        let toks = tokens("<!-- atomik:{\"node\":\"callout\"} -->");
        assert_eq!(toks, vec![Token::Annotation("{\"node\":\"callout\"}")]);

        let toks = tokens("<!-- just a comment --><div>x</div><aside>💡 hi</aside>");
        assert_eq!(toks[0], Token::Raw("<div>x</div>"));
        assert!(matches!(&toks[1], Token::Container(t) if t.name == "aside"));
        assert_eq!(toks[2], Token::Raw("💡 hi"));
        assert!(matches!(&toks[3], Token::Container(t) if t.closing));
    }

    #[test]
    fn unterminated_comments_and_stray_angles_do_not_loop() {
        assert_eq!(tokens("<!-- atomik:{}"), vec![Token::Annotation("{}")]);
        assert_eq!(tokens("a < b <<< c"), vec![Token::Raw("a < b <<< c")]);
        assert!(tokens("").is_empty());
    }
}
