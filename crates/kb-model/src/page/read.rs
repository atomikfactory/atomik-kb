//! Reading the Loro shape of a page into an owned tree.
//!
//! Every reader of [`super::PageDoc`] starts here, so they all agree on what the
//! document contains. The walk is iterative and stops at [`MAX_DEPTH`]; anything it
//! cannot interpret (a map without a string `nodeName`, a plain value or a foreign
//! container in a children list, nesting deeper than the limit) becomes
//! [`Child::Skipped`] and is reported in [`Tree::problems`], never a panic.
//!
//! `Child` entries mirror the engine's children lists one to one, so the position
//! of a child in [`Element::children`] is its index in the `LoroList`.

use kb_types::{AccountId, AttachmentId, BlockId, PageId};
use loro::{Container, LoroMap, LoroText, LoroValue, TextDelta, ValueOrContainer};

use super::{date, schema, shape};

/// Node type of text in ProseMirror JSON (never an element node name).
const TEXT: &str = "text";

/// Deepest element nesting the readers follow (the root is depth 0). Real pages stay
/// far below; portable JSON parsed from text cannot even hold more than about 60
/// levels (`serde_json` stops at 128 nested arrays and objects).
pub(super) const MAX_DEPTH: usize = 128;

/// How much of the text runs to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    /// Element structure and attributes only; text runs are not read.
    Structure,
    /// Text content without marks.
    Plain,
    /// Text content with marks.
    Rich,
}

/// A page document as read from the engine.
pub(super) struct Tree {
    /// The root node; `None` while the engine document has no root yet.
    pub root: Option<Element>,
    /// Shape problems met while reading, for `check_invariants`. Content-free: node
    /// positions and fixed messages only.
    pub problems: Vec<String>,
}

/// An element node (`LoroMap { nodeName, attributes, children }`).
pub(super) struct Element {
    /// The engine container of the node.
    pub map: LoroMap,
    /// `nodeName`.
    pub name: String,
    /// Non-null attributes sorted by key (container values read deeply).
    pub attrs: Vec<(String, LoroValue)>,
    /// One entry per entry of the `children` list, in order.
    pub children: Vec<Child>,
}

/// One entry of a gallery's `items`.
pub(super) struct GalleryEntry<'a> {
    /// The attachment, when `attachmentId` is a valid id.
    pub attachment: Option<AttachmentId>,
    /// The caption, when it is a string (possibly empty).
    pub caption: Option<&'a str>,
    /// A map with a valid `attachmentId` and a caption that is absent or a string.
    pub well_formed: bool,
}

/// One entry of a children list.
pub(super) enum Child {
    /// A nested element node.
    Element(Element),
    /// A `LoroText`: its non-empty runs (always empty in [`Mode::Structure`]).
    Text(Vec<Segment>),
    /// Anything readers ignore (reported in [`Tree::problems`]).
    Skipped,
}

/// A run of text with one set of marks.
pub(super) struct Segment {
    /// The text (never empty).
    pub text: String,
    /// Marks in canonical order (see [`mark_order`]); empty in [`Mode::Plain`].
    pub marks: Vec<Mark>,
}

/// A text mark with its attributes.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Mark {
    /// Mark name (`bold`, `link`, ...).
    pub name: String,
    /// Non-null attributes sorted by key; empty when the mark has none.
    pub attrs: Vec<(String, LoroValue)>,
}

impl Element {
    /// A plain attribute value.
    pub fn attr(&self, key: &str) -> Option<&LoroValue> {
        self.attrs
            .binary_search_by(|(k, _)| k.as_str().cmp(key))
            .ok()
            .and_then(|i| self.attrs.get(i))
            .map(|(_, value)| value)
    }

    /// A string attribute.
    pub fn str_attr(&self, key: &str) -> Option<&str> {
        match self.attr(key)? {
            LoroValue::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// The block id, when `attributes.id` is a valid UUID string.
    pub fn block_id(&self) -> Option<BlockId> {
        self.str_attr(shape::ID)?.parse().ok()
    }

    /// The referenced page of a `pageRef` or a `pageLink`.
    pub fn ref_page(&self) -> Option<PageId> {
        self.str_attr(shape::PAGE_ID)?.parse().ok()
    }

    /// The referenced block of a `pageRef` or a `pageLink` (absent or malformed: none).
    pub fn ref_block(&self) -> Option<BlockId> {
        self.str_attr(shape::BLOCK_ID)?.parse().ok()
    }

    /// What a `dateMention` reads as: its `date`, then a space and its `time` when it has
    /// one (`2026-10-07`, `2026-10-07 14:30`). `None` unless `date` is a valid date; a
    /// malformed `time` is left out (see `super::date`).
    pub fn date_text(&self) -> Option<String> {
        date::iso_text(self.str_attr(shape::DATE), self.str_attr(shape::TIME))
    }

    /// The `reminder` of a `dateMention`: minutes before its moment, a non-negative
    /// integer. `None` when it is absent, and also when it is not one (negative, fractional,
    /// not a number): `check_invariants` tells the two apart, and readers treat a damaged
    /// reminder as no reminder.
    pub fn reminder_minutes(&self) -> Option<i64> {
        let minutes = match self.attr(shape::REMINDER)? {
            LoroValue::I64(minutes) => *minutes,
            LoroValue::Double(minutes) => integral(*minutes)?,
            _ => return None,
        };
        (minutes >= 0).then_some(minutes)
    }

    /// The `reminderFor` of a `dateMention`: the account whose devices fire the reminder (ADR-0047),
    /// a valid account id. `None` when it is absent, and also when it is not one:
    /// `check_invariants` tells the two apart, and readers treat a damaged value as absent.
    pub fn reminder_for(&self) -> Option<AccountId> {
        self.str_attr(shape::REMINDER_FOR)?.parse().ok()
    }

    /// The `relative` of a `dateMention` (ADR-0056): `"today"` or `"now"`. `None` when it is absent, and also
    /// when it is anything else: `check_invariants` tells the two apart, and readers treat a damaged value as absent.
    pub fn date_relative(&self) -> Option<&str> {
        self.str_attr(shape::RELATIVE)
            .filter(|relative| matches!(*relative, "today" | "now"))
    }

    /// The `relative` of a `personMention` (ADR-0056): `"me"`. `None` when it is absent or anything else.
    pub fn person_relative(&self) -> Option<&str> {
        self.str_attr(shape::RELATIVE)
            .filter(|relative| matches!(*relative, "me"))
    }

    /// The `position` of a `templateButton` (ADR-0056): `"below"` or `"above"`. `None` when it is absent (below),
    /// and also when it is anything else.
    pub fn button_position(&self) -> Option<&str> {
        self.str_attr(shape::POSITION)
            .filter(|position| matches!(*position, "below" | "above"))
    }

    /// The `accountId` of a `personMention`: a valid account id. `None` when it is absent or not one.
    pub fn account_id(&self) -> Option<AccountId> {
        self.str_attr(shape::ACCOUNT_ID)?.parse().ok()
    }

    /// The LaTeX source of a `mathInline`: its `latex` attribute when that is a string (empty
    /// while the equation is being written). `None` when it is absent or not a string.
    pub fn latex(&self) -> Option<&str> {
        self.str_attr(shape::LATEX)
    }

    /// The attachment of an `image` or `file` block (absent or malformed: none).
    pub fn attachment_id(&self) -> Option<AttachmentId> {
        self.str_attr(shape::ATTACHMENT_ID)?.parse().ok()
    }

    /// The `widthPercent` of an `image`: an integer from 10 to 100 (`50.0` is 50). `None`
    /// when it is absent, and also when it is not one (out of range, fractional, not a
    /// number): `check_invariants` tells the two apart, and readers treat a damaged value
    /// as absent.
    pub fn image_width_percent(&self) -> Option<u8> {
        let percent = match self.attr(shape::WIDTH_PERCENT)? {
            LoroValue::I64(percent) => *percent,
            LoroValue::Double(percent) => integral(*percent)?,
            _ => return None,
        };
        u8::try_from(percent)
            .ok()
            .filter(|percent| (10..=100).contains(percent))
    }

    /// The `align` of an `image`: `"left"`, `"center"` or `"right"`. `None` when it is
    /// absent or anything else.
    pub fn image_align(&self) -> Option<&str> {
        self.str_attr(shape::ALIGN)
            .filter(|align| matches!(*align, "left" | "center" | "right"))
    }

    /// The `view` of a `file`: `"preview"` (shown as an inline viewer). `None` when it is
    /// absent (the file card) or anything else: `check_invariants` tells the two apart, and
    /// readers treat a damaged value as absent.
    pub fn file_view(&self) -> Option<&str> {
        self.str_attr(shape::VIEW)
            .filter(|view| matches!(*view, "preview"))
    }

    /// The `caption` of an `image` when it is a string (possibly empty). `None` when it is
    /// absent or not a string.
    pub fn image_caption(&self) -> Option<&str> {
        self.str_attr(shape::CAPTION)
    }

    /// The `columns` of a `gallery`: an integer from 2 to 4 (`3.0` is 3). `None` when it is
    /// absent, and also when it is not one; readers show three columns then.
    pub fn gallery_columns(&self) -> Option<u8> {
        let columns = match self.attr(shape::COLUMNS)? {
            LoroValue::I64(columns) => *columns,
            LoroValue::Double(columns) => integral(*columns)?,
            _ => return None,
        };
        u8::try_from(columns)
            .ok()
            .filter(|columns| (2..=4).contains(columns))
    }

    /// The `items` of a `gallery`, in order. `None` when the attribute is absent or is not a
    /// list; an entry that is not a map reads as an entry without attachment.
    pub fn gallery_entries(&self) -> Option<Vec<GalleryEntry<'_>>> {
        let LoroValue::List(items) = self.attr(shape::ITEMS)? else {
            return None;
        };
        Some(
            items
                .iter()
                .map(|item| {
                    let attachment = match item.get_by_key(shape::ATTACHMENT_ID) {
                        Some(LoroValue::String(id)) => id.as_str().parse::<AttachmentId>().ok(),
                        _ => None,
                    };
                    let (caption, caption_ok) = match item.get_by_key(shape::CAPTION) {
                        None => (None, true),
                        Some(LoroValue::String(caption)) => (Some(caption.as_str()), true),
                        Some(_) => (None, false),
                    };
                    GalleryEntry {
                        attachment,
                        caption,
                        well_formed: matches!(item, LoroValue::Map(_))
                            && attachment.is_some()
                            && caption_ok,
                    }
                })
                .collect(),
        )
    }

    /// The `height` of a `whiteboard`: an integer from 160 to 1200 (`360.0` is 360). `None` when
    /// it is absent, and also when it is not one; readers show the default height then.
    pub fn whiteboard_height(&self) -> Option<u32> {
        let height = match self.attr(shape::HEIGHT)? {
            LoroValue::I64(height) => *height,
            LoroValue::Double(height) => integral(*height)?,
            _ => return None,
        };
        u32::try_from(height)
            .ok()
            .filter(|height| (160..=1200).contains(height))
    }

    /// The `level` of a `detailsSummary`: an integer from 1 to 3 (`2.0` is 2), which makes its
    /// toggle a toggle heading. `None` when it is absent (a plain toggle), and also when it is
    /// not one (out of range, fractional, not a number): `check_invariants` tells the two
    /// apart, and readers treat a damaged level as absent.
    pub fn summary_level(&self) -> Option<u8> {
        let level = match self.attr(shape::LEVEL)? {
            LoroValue::I64(level) => *level,
            LoroValue::Double(level) => integral(*level)?,
            _ => return None,
        };
        u8::try_from(level)
            .ok()
            .filter(|level| (1..=3).contains(level))
    }

    /// Whether this node has the given name.
    pub fn is(&self, name: &str) -> bool {
        self.name == name
    }
}

/// `value` as an `i64` when it is integral and in range (the rule loro-crdt applies
/// to JavaScript numbers).
pub(super) fn integral(value: f64) -> Option<i64> {
    // `i64::MAX as f64` rounds up to 2^63; the cast saturates there, like loro-crdt.
    let (min, max) = (i64::MIN as f64, i64::MAX as f64);
    if value.is_finite() && value.fract() == 0.0 && value >= min && value <= max {
        Some(value as i64)
    } else {
        None
    }
}

/// Canonical mark order: the v1 marks in table order, then unknown marks by name.
pub(super) fn mark_order(a: &Mark, b: &Mark) -> std::cmp::Ordering {
    mark_rank(&a.name)
        .cmp(&mark_rank(&b.name))
        .then_with(|| a.name.cmp(&b.name))
}

fn mark_rank(name: &str) -> usize {
    schema::MARKS
        .iter()
        .position(|(known, _)| *known == name)
        .unwrap_or(schema::MARKS.len())
}

/// Reads the whole document.
pub(super) fn read(root: &LoroMap, mode: Mode) -> Tree {
    let mut problems = Vec::new();
    if root.is_empty() {
        return Tree {
            root: None,
            problems,
        };
    }
    let root = walk(root, 0, mode, &mut problems);
    Tree { root, problems }
}

/// Reads the subtree of one element node found at `depth` (problems are dropped).
/// `None` when the map is not a readable node.
pub(super) fn read_element(map: &LoroMap, depth: usize, mode: Mode) -> Option<Element> {
    let mut problems = Vec::new();
    walk(map, depth, mode, &mut problems)
}

struct Frame {
    element: Element,
    items: std::vec::IntoIter<ValueOrContainer>,
    next: usize,
}

/// Iterative pre-order walk from `start`, which sits at `depth` (0 = the root).
fn walk(start: &LoroMap, depth: usize, mode: Mode, problems: &mut Vec<String>) -> Option<Element> {
    // Child positions of the frames below `start`, for problem messages.
    let mut path: Vec<usize> = Vec::new();
    let (element, items) = header(start, depth == 0, &path, problems)?;
    let mut stack = vec![Frame {
        element,
        items: items.into_iter(),
        next: 0,
    }];
    let mut done = None;
    while let Some(frame) = stack.last_mut() {
        let Some(item) = frame.items.next() else {
            if let Some(finished) = stack.pop() {
                path.pop();
                match stack.last_mut() {
                    Some(parent) => parent
                        .element
                        .children
                        .push(Child::Element(finished.element)),
                    None => done = Some(finished.element),
                }
            }
            continue;
        };
        let index = frame.next;
        frame.next += 1;
        path.push(index);
        let child = match item {
            ValueOrContainer::Container(Container::Map(map)) => {
                if depth + stack.len() > MAX_DEPTH {
                    problems.push(format!(
                        "{}: nested deeper than {MAX_DEPTH} levels",
                        at(&path)
                    ));
                    Child::Skipped
                } else if let Some((element, items)) = header(&map, false, &path, problems) {
                    // The path entry stays until the frame is finished.
                    stack.push(Frame {
                        element,
                        items: items.into_iter(),
                        next: 0,
                    });
                    continue;
                } else {
                    Child::Skipped
                }
            }
            ValueOrContainer::Container(Container::Text(text)) => {
                Child::Text(segments(&text, mode))
            }
            ValueOrContainer::Container(_) | ValueOrContainer::Value(_) => {
                problems.push(format!(
                    "{}: children entry is neither a node nor a text",
                    at(&path)
                ));
                Child::Skipped
            }
        };
        path.pop();
        if let Some(frame) = stack.last_mut() {
            frame.element.children.push(child);
        }
    }
    done
}

/// Position of a node for problem messages: `root` or `node /i/j` (child indices).
pub(super) fn at(path: &[usize]) -> String {
    if path.is_empty() {
        return "root".to_owned();
    }
    let mut out = String::from("node ");
    for index in path {
        out.push('/');
        out.push_str(&index.to_string());
    }
    out
}

/// Reads a node's own fields and the entries of its children list.
fn header(
    map: &LoroMap,
    is_root: bool,
    path: &[usize],
    problems: &mut Vec<String>,
) -> Option<(Element, Vec<ValueOrContainer>)> {
    let name = match map.get(shape::NODE_NAME) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) if s.is_empty() => {
            problems.push(format!("{}: empty nodeName", at(path)));
            None
        }
        // Text is a `LoroText`; a map claiming to be text is malformed.
        Some(ValueOrContainer::Value(LoroValue::String(s))) if s.as_str() == TEXT => {
            problems.push(format!("{}: element node named text", at(path)));
            None
        }
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.as_str().to_owned()),
        Some(_) => {
            problems.push(format!("{}: nodeName is not a string", at(path)));
            None
        }
        None => {
            problems.push(format!("{}: no nodeName", at(path)));
            None
        }
    };
    let name = match name {
        Some(name) => name,
        // The root is the page whatever it says; other nodes need a name.
        None if is_root => shape::DOC.to_owned(),
        None => return None,
    };
    if is_root && name != shape::DOC {
        problems.push("root: nodeName is not \"doc\"".to_owned());
    }
    let attrs = match map.get(shape::ATTRIBUTES) {
        Some(ValueOrContainer::Container(Container::Map(attributes))) => {
            read_attributes(&attributes)
        }
        Some(_) => {
            problems.push(format!("{}: attributes is not a map", at(path)));
            Vec::new()
        }
        None => {
            problems.push(format!("{}: no attributes map", at(path)));
            Vec::new()
        }
    };
    let items = match map.get(shape::CHILDREN) {
        Some(ValueOrContainer::Container(Container::List(list))) => {
            let mut items = Vec::with_capacity(list.len());
            list.for_each(|item| items.push(item));
            items
        }
        Some(_) => {
            problems.push(format!("{}: children is not a list", at(path)));
            Vec::new()
        }
        None => {
            problems.push(format!("{}: no children list", at(path)));
            Vec::new()
        }
    };
    let element = Element {
        map: map.clone(),
        name,
        attrs,
        children: Vec::with_capacity(items.len()),
    };
    Some((element, items))
}

fn read_attributes(map: &LoroMap) -> Vec<(String, LoroValue)> {
    let mut entries = Vec::with_capacity(map.len());
    map.for_each(|key, value| entries.push((key.to_owned(), value)));
    let mut attrs: Vec<(String, LoroValue)> = entries
        .into_iter()
        .map(|(key, value)| (key, value.get_deep_value()))
        .filter(|(_, value)| !matches!(value, LoroValue::Null))
        .collect();
    attrs.sort_by(|a, b| a.0.cmp(&b.0));
    attrs
}

fn segments(text: &LoroText, mode: Mode) -> Vec<Segment> {
    match mode {
        Mode::Structure => Vec::new(),
        Mode::Plain => {
            let text = text.to_string();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![Segment {
                    text,
                    marks: Vec::new(),
                }]
            }
        }
        Mode::Rich => text
            .to_delta()
            .into_iter()
            .filter_map(|delta| match delta {
                TextDelta::Insert { insert, attributes } if !insert.is_empty() => {
                    let mut marks: Vec<Mark> = attributes
                        .into_iter()
                        .flatten()
                        // A null value is what an unmark leaves behind: no mark.
                        .filter(|(_, value)| !matches!(value, LoroValue::Null))
                        .map(|(name, value)| Mark {
                            attrs: mark_attrs(&value),
                            name,
                        })
                        .collect();
                    marks.sort_by(mark_order);
                    Some(Segment {
                        text: insert,
                        marks,
                    })
                }
                _ => None,
            })
            .collect(),
    }
}

/// A mark value is the mark's attributes as a map (`{}` without attributes); any
/// other non-null value is read as a mark without attributes.
fn mark_attrs(value: &LoroValue) -> Vec<(String, LoroValue)> {
    match value {
        LoroValue::Map(map) => {
            let mut attrs: Vec<(String, LoroValue)> = map
                .iter()
                .filter(|(_, value)| !matches!(value, LoroValue::Null))
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            attrs.sort_by(|a, b| a.0.cmp(&b.0));
            attrs
        }
        _ => Vec::new(),
    }
}
