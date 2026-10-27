//! YAML front matter: cutting it off a Markdown file and reading it into plain values.
//!
//! The parser of `yaml-rust2` runs in its event form and the tree is built here with a depth
//! limit, so a hostile document cannot recurse deeply (neither while it is read nor when it is
//! dropped), and aliases are never expanded (an alias reads as null).

use yaml_rust2::Event;
use yaml_rust2::parser::{MarkedEventReceiver, Parser};
use yaml_rust2::scanner::{Marker, TScalarStyle};

use crate::values;

/// The largest front matter read, in bytes.
pub(crate) const MAX_FRONT_MATTER: usize = 64 * 1024;
/// Nesting deeper than this reads as null.
const MAX_DEPTH: usize = 8;
/// Most nodes kept.
const MAX_NODES: usize = 20_000;

/// A YAML value, reduced to what properties need.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Yv {
    /// `null`, `~`, an empty value, an alias, something nested too deeply.
    Null,
    /// A boolean.
    Bool(bool),
    /// A number.
    Number(f64),
    /// A string.
    Str(String),
    /// A sequence.
    Seq(Vec<Yv>),
    /// A mapping with string keys, in document order.
    Map(Vec<(String, Yv)>),
}

impl Yv {
    /// A scalar as text; `None` for null, a sequence or a mapping.
    pub(crate) fn scalar_text(&self) -> Option<String> {
        match self {
            Yv::Bool(b) => Some(b.to_string()),
            Yv::Number(n) => Some(values::number_text(*n)),
            Yv::Str(s) => Some(s.clone()),
            Yv::Null | Yv::Seq(_) | Yv::Map(_) => None,
        }
    }

    /// The value as one line of YAML-like text (for a value that fits no property kind).
    pub(crate) fn to_text(&self) -> String {
        let mut out = String::new();
        self.write_text(&mut out, 0);
        out
    }

    fn write_text(&self, out: &mut String, depth: usize) {
        if depth > MAX_DEPTH + 1 {
            return;
        }
        match self {
            Yv::Null => {}
            Yv::Bool(b) => out.push_str(&b.to_string()),
            Yv::Number(n) => out.push_str(&values::number_text(*n)),
            Yv::Str(s) => out.push_str(s),
            Yv::Seq(items) => {
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    item.write_text(out, depth + 1);
                }
            }
            Yv::Map(pairs) => {
                for (index, (key, value)) in pairs.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(key);
                    out.push_str(": ");
                    value.write_text(out, depth + 1);
                }
            }
        }
    }
}

/// A Markdown file cut into its front matter and its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Split<'a> {
    /// The YAML between the `---` lines.
    pub yaml: Option<&'a str>,
    /// The rest of the file.
    pub body: &'a str,
}

/// Cuts front matter off: a first line `---` closed by the next line `---` or `...`. Without a
/// closing line the file has none.
pub(crate) fn split(text: &str) -> Split<'_> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let none = Split {
        yaml: None,
        body: text,
    };
    let Some(first_end) = text.find('\n') else {
        return none;
    };
    if text[..first_end].trim_end() != "---" {
        return none;
    }
    let start = first_end + 1;
    let mut at = start;
    while at <= text.len() {
        let line_end = text[at..].find('\n').map_or(text.len(), |n| at + n);
        let line = text[at..line_end].trim_end();
        if line == "---" || line == "..." {
            let body_start = (line_end + 1).min(text.len());
            return Split {
                yaml: Some(&text[start..at]),
                body: &text[body_start..],
            };
        }
        if line_end >= text.len() {
            break;
        }
        at = line_end + 1;
    }
    none
}

/// Reads front matter into the pairs of its top-level mapping. `None` when it is not valid YAML,
/// is too large or is not a mapping.
pub(crate) fn parse(yaml: &str) -> Option<Vec<(String, Yv)>> {
    if yaml.len() > MAX_FRONT_MATTER {
        return None;
    }
    if yaml.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut builder = Builder::default();
    let mut parser = Parser::new_from_str(yaml);
    // Events are pulled one at a time: the parser's own `load` recurses once per level of nesting,
    // which a hostile document turns into a stack overflow.
    loop {
        let (event, mark) = parser.next_token().ok()?;
        let last = matches!(event, Event::StreamEnd | Event::DocumentEnd);
        builder.on_event(event, mark);
        if last || builder.failed {
            break;
        }
    }
    if builder.failed {
        return None;
    }
    match builder.root? {
        Yv::Map(pairs) => Some(pairs),
        Yv::Null => Some(Vec::new()),
        _ => None,
    }
}

enum Frame {
    Seq(Vec<Yv>),
    Map {
        pairs: Vec<(String, Yv)>,
        key: Option<Yv>,
    },
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    root: Option<Yv>,
    /// Events to skip: the nesting depth inside a collection that was cut off.
    skipping: usize,
    nodes: usize,
    failed: bool,
}

impl Builder {
    fn attach(&mut self, value: Yv) {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            self.failed = true;
            return;
        }
        match self.stack.last_mut() {
            None => self.root = Some(value),
            Some(Frame::Seq(items)) => items.push(value),
            Some(Frame::Map { pairs, key }) => match key.take() {
                None => *key = Some(value),
                Some(k) => {
                    if let Some(name) = key_text(&k) {
                        // A repeated key: the last one wins.
                        pairs.retain(|(existing, _)| *existing != name);
                        pairs.push((name, value));
                    }
                }
            },
        }
    }
}

fn key_text(key: &Yv) -> Option<String> {
    key.scalar_text().map(|text| text.trim().to_owned())
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, event: Event, _mark: Marker) {
        if self.failed {
            return;
        }
        if self.skipping > 0 {
            match event {
                Event::SequenceStart(..) | Event::MappingStart(..) => self.skipping += 1,
                Event::SequenceEnd | Event::MappingEnd => {
                    self.skipping -= 1;
                    if self.skipping == 0 {
                        self.attach(Yv::Null);
                    }
                }
                _ => {}
            }
            return;
        }
        match event {
            Event::Scalar(text, style, _, tag) => {
                let value = if style == TScalarStyle::Plain && tag.is_none() {
                    plain_scalar(&text)
                } else {
                    Yv::Str(text)
                };
                self.attach(value);
            }
            Event::Alias(_) => self.attach(Yv::Null),
            Event::SequenceStart(..) => {
                if self.stack.len() >= MAX_DEPTH {
                    self.skipping = 1;
                } else {
                    self.stack.push(Frame::Seq(Vec::new()));
                }
            }
            Event::MappingStart(..) => {
                if self.stack.len() >= MAX_DEPTH {
                    self.skipping = 1;
                } else {
                    self.stack.push(Frame::Map {
                        pairs: Vec::new(),
                        key: None,
                    });
                }
            }
            Event::SequenceEnd | Event::MappingEnd => {
                let value = match self.stack.pop() {
                    Some(Frame::Seq(items)) => Yv::Seq(items),
                    Some(Frame::Map { pairs, .. }) => Yv::Map(pairs),
                    None => return,
                };
                self.attach(value);
            }
            _ => {}
        }
    }
}

/// The value of a plain (unquoted) scalar: null, a boolean, a number, else a string. Numbers
/// with a leading zero (`007`) and the YAML 1.1 words (`yes`, `no`, `on`, `off`) stay strings.
fn plain_scalar(text: &str) -> Yv {
    let trimmed = text.trim();
    match trimmed {
        "" | "~" | "null" | "Null" | "NULL" => return Yv::Null,
        "true" | "True" | "TRUE" => return Yv::Bool(true),
        "false" | "False" | "FALSE" => return Yv::Bool(false),
        _ => {}
    }
    match values::parse_number(trimmed) {
        Some(number) => Yv::Number(number),
        None => Yv::Str(text.to_owned()),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn pairs(yaml: &str) -> Vec<(String, Yv)> {
        parse(yaml).expect("valid YAML")
    }

    #[test]
    fn front_matter_is_cut_off() {
        let text = "---\ntitle: A\n---\nbody\n";
        assert_eq!(
            split(text),
            Split {
                yaml: Some("title: A\n"),
                body: "body\n"
            }
        );
        let text = "\u{feff}---\r\ntitle: A\r\n...\r\nbody";
        assert_eq!(
            split(text),
            Split {
                yaml: Some("title: A\r\n"),
                body: "body"
            }
        );
        assert_eq!(split("---\n---\nx").yaml, Some(""));
        assert_eq!(split("---\ntitle: A\n").yaml, None);
        assert_eq!(split("--- \na: 1\n--- \nrest").body, "rest");
        let plain = "# Title\n\n---\nnot front matter\n---\n";
        assert_eq!(
            split(plain),
            Split {
                yaml: None,
                body: plain
            }
        );
        assert_eq!(split("---").yaml, None);
        assert_eq!(split("").body, "");
        // A closing line at the very end of the file.
        assert_eq!(
            split("---\na: 1\n---"),
            Split {
                yaml: Some("a: 1\n"),
                body: ""
            }
        );
    }

    #[test]
    fn scalars_lists_and_maps() {
        let got = pairs(
            "title: \"Weekly: review\"\nn: 3.5\ni: 7\nb: true\nz: ~\ns: plain text\nzip: 007\n\
             list:\n  - a\n  - 2\n  - \"x y\"\nplace:\n  name: Paris\n  lat: 48.8566\nempty:\nquoted: '3'\n",
        );
        let map: std::collections::HashMap<_, _> = got.into_iter().collect();
        assert_eq!(map["title"], Yv::Str("Weekly: review".into()));
        assert_eq!(map["n"], Yv::Number(3.5));
        assert_eq!(map["i"], Yv::Number(7.0));
        assert_eq!(map["b"], Yv::Bool(true));
        assert_eq!(map["z"], Yv::Null);
        assert_eq!(map["s"], Yv::Str("plain text".into()));
        assert_eq!(map["zip"], Yv::Str("007".into()));
        assert_eq!(
            map["list"],
            Yv::Seq(vec![
                Yv::Str("a".into()),
                Yv::Number(2.0),
                Yv::Str("x y".into())
            ])
        );
        assert_eq!(
            map["place"],
            Yv::Map(vec![
                ("name".into(), Yv::Str("Paris".into())),
                ("lat".into(), Yv::Number(48.8566))
            ])
        );
        assert_eq!(map["empty"], Yv::Null);
        assert_eq!(map["quoted"], Yv::Str("3".into()));
    }

    #[test]
    fn order_and_repeated_keys() {
        let got = pairs("b: 1\na: 2\nb: 3\n");
        assert_eq!(
            got,
            vec![("a".into(), Yv::Number(2.0)), ("b".into(), Yv::Number(3.0))]
        );
    }

    #[test]
    fn invalid_or_hostile_yaml_is_refused() {
        assert!(parse("a: [unclosed").is_none());
        assert!(parse("- just\n- a list\n").is_none());
        assert!(parse("plain scalar").is_none());
        assert!(parse(&format!("a: {}", "x".repeat(MAX_FRONT_MATTER))).is_none());
        // Aliases are not expanded.
        let got = pairs("a: &x [1, 2]\nb: *x\n");
        assert_eq!(got[1], ("b".into(), Yv::Null));
        // Deep nesting reads as null and costs nothing.
        let deep = format!("a: {}{}\nb: 1\n", "[".repeat(5_000), "]".repeat(5_000));
        if let Some(got) = parse(&deep) {
            assert!(got.iter().any(|(key, _)| key == "b") || got.is_empty());
        }
        let many = (0..30_000)
            .map(|n| format!("k{n}: v\n"))
            .collect::<String>();
        assert!(parse(&many).is_none());
    }

    #[test]
    fn nested_values_read_as_text() {
        let got = pairs("m:\n  a: 1\n  b:\n    - x\n    - y\n");
        assert_eq!(got[0].1.to_text(), "a: 1, b: x, y");
    }
}
