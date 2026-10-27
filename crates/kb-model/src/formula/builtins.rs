//! The functions of `formulas.md` section 3 over values. An empty number, date or boolean argument
//! makes the result empty (arithmetic and dates propagate); an empty text reads as `""` and an
//! empty list as `[]` in the text and list functions.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

use kb_types::properties::{MAX_FORMULA_LIST_ITEMS, MAX_TEXT_CHARS};
use kb_types::{AccountId, PageId};
use regex::Regex;

use super::dates;
use super::eval::{Eval, Ran, capped_text, count_value};
use super::format::{number_to_string, parse_number};
use super::functions::F;
use super::regex_js;
use super::value::{Date, Value, equal};

/// Texts longer than this many bytes are not worth building (the cap is in characters, and a
/// character takes at most four bytes).
const MAX_TEXT_BYTES: usize = MAX_TEXT_CHARS * 4;

fn num(args: &[Value], index: usize) -> Option<f64> {
    args.get(index).and_then(Value::as_number)
}

fn text(args: &[Value], index: usize) -> &str {
    args.get(index).map_or("", Value::as_str)
}

fn items(args: &[Value], index: usize) -> &[Value] {
    args.get(index).map_or(&[][..], Value::items)
}

fn date(args: &[Value], index: usize) -> Option<Date> {
    args.get(index).and_then(Value::as_date)
}

/// An argument as a whole number (truncated).
fn whole(args: &[Value], index: usize) -> Option<i64> {
    num(args, index).and_then(dates::to_i64)
}

/// A whole number as a value.
#[allow(clippy::cast_precision_loss)]
fn int_value(value: i64) -> Value {
    Value::Number(value as f64)
}

/// `empty()` (section 3.1): empty, `""`, `[]`, `0` and `false`.
fn is_empty_value(value: &Value) -> bool {
    match value {
        Value::Empty => true,
        Value::Text(text) => text.is_empty(),
        Value::List(items) => items.is_empty(),
        Value::Number(number) => *number == 0.0,
        Value::Bool(flag) => !*flag,
        _ => false,
    }
}

/// A position clamped into `0..=len` (a number is truncated; a negative one is 0).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn clamp_index(value: f64, len: usize) -> usize {
    let whole = value.trunc();
    if whole <= 0.0 {
        0
    } else if whole >= count_value(len) {
        len
    } else {
        whole as usize
    }
}

/// `at`: a position from the start or, when negative, from the end.
fn pick(list: &[Value], index: i64) -> Value {
    let len = i64::try_from(list.len()).unwrap_or(i64::MAX);
    let position = if index < 0 { len + index } else { index };
    usize::try_from(position)
        .ok()
        .and_then(|i| list.get(i))
        .cloned()
        .unwrap_or(Value::Empty)
}

/// The sort key of a list item.
enum SortKey {
    Empty,
    Number(f64),
    Text(Arc<str>),
    Bool(bool),
    Date(i64),
    Named(String, String),
}

fn compare_keys(a: &SortKey, b: &SortKey) -> Ordering {
    match (a, b) {
        (SortKey::Empty, SortKey::Empty) => Ordering::Equal,
        (SortKey::Empty, _) => Ordering::Greater,
        (_, SortKey::Empty) => Ordering::Less,
        (SortKey::Number(x), SortKey::Number(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
        (SortKey::Text(x), SortKey::Text(y)) => x.as_ref().cmp(y.as_ref()),
        (SortKey::Bool(x), SortKey::Bool(y)) => x.cmp(y),
        (SortKey::Date(x), SortKey::Date(y)) => x.cmp(y),
        (SortKey::Named(n1, i1), SortKey::Named(n2, i2)) => n1.cmp(n2).then_with(|| i1.cmp(i2)),
        _ => Ordering::Equal,
    }
}

/// What makes two items "the same" for `unique`.
#[derive(Hash, PartialEq, Eq)]
enum Identity {
    Empty,
    Number(u64),
    Text(Arc<str>),
    Bool(bool),
    Date(i64),
    Page(PageId),
    Person(AccountId),
    /// A list item: never merged (lists do not nest after checking).
    Distinct(usize),
}

fn identity(value: &Value, position: usize) -> Identity {
    match value {
        Value::Empty => Identity::Empty,
        Value::Number(number) => {
            Identity::Number(if *number == 0.0 { 0 } else { number.to_bits() })
        }
        Value::Text(text) => Identity::Text(Arc::clone(text)),
        Value::Bool(flag) => Identity::Bool(*flag),
        Value::Date(date) => Identity::Date(date.ms),
        Value::Page(page) => Identity::Page(page.id),
        Value::Person(account) => Identity::Person(*account),
        Value::List(_) => Identity::Distinct(position),
    }
}

impl Eval<'_> {
    /// `format` (section 3.4.2) of any value.
    pub(super) fn format(&self, value: &Value) -> String {
        match value {
            Value::Empty => String::new(),
            Value::Number(number) => number_to_string(*number),
            Value::Text(text) => text.to_string(),
            Value::Bool(flag) => flag.to_string(),
            Value::Date(date) => dates::format_date(
                date.ms,
                if date.time {
                    "MMMM D, YYYY h:mm A"
                } else {
                    "MMMM D, YYYY"
                },
            ),
            Value::Page(page) => self.host.title(page.id).unwrap_or_default(),
            Value::Person(account) => self
                .host
                .person(*account)
                .map(|(name, _)| name)
                .unwrap_or_default(),
            Value::List(items) => items
                .iter()
                .map(|item| self.format(item))
                .collect::<Vec<_>>()
                .join(", "),
        }
    }

    /// A function that is not compiled to a node of its own.
    pub(super) fn call(&mut self, function: F, args: &[Value]) -> Ran<Value> {
        match function {
            F::Not => Ok(Value::Bool(!args.first().is_some_and(Value::truthy))),
            F::Empty => Ok(Value::Bool(args.first().is_none_or(is_empty_value))),
            F::Add
            | F::Subtract
            | F::Multiply
            | F::Divide
            | F::Mod
            | F::Pow
            | F::Abs
            | F::Ceil
            | F::Floor
            | F::Sqrt
            | F::Cbrt
            | F::Exp
            | F::Ln
            | F::Log10
            | F::Log2
            | F::Sign
            | F::Pi
            | F::E
            | F::Round
            | F::ToNumber => Ok(self.numeric(function, args)),
            F::Min | F::Max | F::Sum | F::Mean | F::Median => self.aggregate(function, args),
            F::Length
            | F::Substring
            | F::Contains
            | F::Test
            | F::Match
            | F::Replace
            | F::ReplaceAll
            | F::Split
            | F::Join
            | F::Lower
            | F::Upper
            | F::Trim
            | F::Repeat
            | F::PadStart
            | F::PadEnd
            | F::Format => self.text_function(function, args),
            F::Now
            | F::Today
            | F::Minute
            | F::Hour
            | F::Day
            | F::Date
            | F::Week
            | F::Month
            | F::Year
            | F::DateAdd
            | F::DateSubtract
            | F::DateBetween
            | F::FormatDate
            | F::ParseDate
            | F::Timestamp
            | F::FromTimestamp => Ok(self.date_function(function, args)),
            F::At
            | F::First
            | F::Last
            | F::Slice
            | F::Concat
            | F::Sort
            | F::Reverse
            | F::Unique
            | F::Includes
            | F::Flat => self.list_builtin(function, args),
            F::Id => Ok(match args.first() {
                None => Value::text(&self.this.to_string()),
                Some(Value::Page(page)) => Value::text(&page.id.to_string()),
                Some(_) => Value::text(""),
            }),
            F::Name | F::Email => Ok(match args.first() {
                Some(Value::Person(account)) => {
                    let found = self.host.person(*account);
                    let (name, email) = found.unwrap_or_default();
                    Value::text(if function == F::Name { &name } else { &email })
                }
                _ => Value::text(""),
            }),
            _ => Ok(Value::Empty),
        }
    }

    // ---- numbers

    fn numeric(&self, function: F, args: &[Value]) -> Value {
        let (x, y) = (num(args, 0), num(args, 1));
        let unary = |op: fn(f64) -> f64| x.map_or(Value::Empty, |a| Value::number(op(a)));
        let binary = |op: fn(f64, f64) -> f64| match (x, y) {
            (Some(a), Some(b)) => Value::number(op(a, b)),
            _ => Value::Empty,
        };
        match function {
            F::Add => binary(|a, b| a + b),
            F::Subtract => binary(|a, b| a - b),
            F::Multiply => binary(|a, b| a * b),
            F::Divide => binary(|a, b| a / b),
            F::Mod => binary(|a, b| a % b),
            F::Pow => binary(f64::powf),
            F::Abs => unary(f64::abs),
            F::Ceil => unary(f64::ceil),
            F::Floor => unary(f64::floor),
            F::Sqrt => unary(f64::sqrt),
            F::Cbrt => unary(f64::cbrt),
            F::Exp => unary(f64::exp),
            F::Ln => unary(f64::ln),
            F::Log10 => unary(f64::log10),
            F::Log2 => unary(f64::log2),
            F::Sign => unary(|a| {
                if a > 0.0 {
                    1.0
                } else if a < 0.0 {
                    -1.0
                } else {
                    0.0
                }
            }),
            F::Pi => Value::Number(std::f64::consts::PI),
            F::E => Value::Number(std::f64::consts::E),
            F::Round => round(args),
            F::ToNumber => self.to_number(args.first()),
            _ => Value::Empty,
        }
    }

    fn to_number(&self, value: Option<&Value>) -> Value {
        match value {
            Some(Value::Number(number)) => Value::Number(*number),
            Some(Value::Text(text)) => parse_number(text).map_or(Value::Empty, Value::number),
            Some(Value::Bool(flag)) => Value::Number(if *flag { 1.0 } else { 0.0 }),
            Some(Value::Date(date)) => int_value(date.ms - self.offset_ms()),
            _ => Value::Empty,
        }
    }

    fn offset_ms(&self) -> i64 {
        i64::from(self.env.offset_seconds) * 1000
    }

    /// `min`, `max`, `sum`, `mean`, `median`: numbers (or dates, for `min` and `max`) from scalars
    /// and lists, empty items skipped.
    fn aggregate(&mut self, function: F, args: &[Value]) -> Ran<Value> {
        let mut numbers: Vec<f64> = Vec::new();
        let mut moments: Vec<Date> = Vec::new();
        let mut visit = |value: &Value| match value {
            Value::Number(number) => numbers.push(*number),
            Value::Date(date) => moments.push(*date),
            _ => {}
        };
        let mut visited = 0_usize;
        for arg in args {
            match arg {
                Value::List(list) => {
                    visited += list.len();
                    list.iter().for_each(&mut visit);
                }
                other => visit(other),
            }
        }
        self.tick(visited)?;
        if !moments.is_empty() {
            let pick = |best: Date, next: &Date| match function {
                F::Max if next.ms > best.ms => *next,
                F::Min if next.ms < best.ms => *next,
                _ => best,
            };
            let first = moments.first().copied();
            return Ok(first.map_or(Value::Empty, |first| {
                Value::Date(moments.iter().skip(1).fold(first, pick))
            }));
        }
        let sum = || numbers.iter().fold(0.0, |total, value| total + value);
        Ok(match function {
            F::Min => numbers
                .iter()
                .copied()
                .reduce(f64::min)
                .map_or(Value::Empty, Value::Number),
            F::Max => numbers
                .iter()
                .copied()
                .reduce(f64::max)
                .map_or(Value::Empty, Value::Number),
            F::Sum => Value::number(sum()),
            F::Mean => {
                if numbers.is_empty() {
                    Value::Empty
                } else {
                    Value::number(sum() / count_value(numbers.len()))
                }
            }
            _ => {
                let mut sorted = numbers.clone();
                sorted.sort_by(f64::total_cmp);
                let middle = sorted.len() / 2;
                match (
                    sorted.get(middle),
                    middle.checked_sub(1).and_then(|i| sorted.get(i)),
                ) {
                    (Some(high), Some(low)) if sorted.len().is_multiple_of(2) => {
                        Value::number((low + high) / 2.0)
                    }
                    (Some(middle), _) => Value::Number(*middle),
                    _ => Value::Empty,
                }
            }
        })
    }

    // ---- text

    fn regex(&mut self, pattern: &str) -> Option<Rc<Regex>> {
        if let Some(known) = self.regexes.get(pattern) {
            return known.clone();
        }
        let compiled = regex_js::compile(pattern).map(Rc::new);
        self.regexes.insert(pattern.to_owned(), compiled.clone());
        compiled
    }

    fn text_function(&mut self, function: F, args: &[Value]) -> Ran<Value> {
        let s = text(args, 0);
        Ok(match function {
            F::Length => Value::Number(count_value(match args.first() {
                Some(Value::List(list)) => list.len(),
                Some(Value::Text(text)) => text.chars().count(),
                _ => 0,
            })),
            F::Substring => {
                let chars: Vec<char> = s.chars().collect();
                let len = chars.len();
                let Some(start) = num(args, 1) else {
                    return Ok(Value::Empty);
                };
                let end = if args.len() > 2 {
                    match num(args, 2) {
                        Some(end) => end,
                        None => return Ok(Value::Empty),
                    }
                } else {
                    count_value(len)
                };
                let (from, to) = (clamp_index(start, len), clamp_index(end, len));
                match chars.get(from..to) {
                    Some(part) if to > from => Value::text(&part.iter().collect::<String>()),
                    _ => Value::text(""),
                }
            }
            F::Contains => Value::Bool(s.contains(text(args, 1))),
            F::Test => match self.regex(text(args, 1)) {
                Some(re) => Value::Bool(re.is_match(s)),
                None => Value::Empty,
            },
            F::Match => {
                let Some(re) = self.regex(text(args, 1)) else {
                    return Ok(Value::Empty);
                };
                let found: Vec<Value> = regex_js::all_captures(&re, s, MAX_FORMULA_LIST_ITEMS + 1)
                    .iter()
                    .filter_map(|captures| captures.get(0))
                    .map(|found| Value::text(found.as_str()))
                    .collect();
                self.tick(found.len())?;
                Value::list(found)
            }
            F::Replace | F::ReplaceAll => self.replace(args, function == F::ReplaceAll),
            F::Split => {
                let separator = text(args, 1);
                let parts: Vec<Value> = if separator.is_empty() {
                    s.chars()
                        .map(|c| Value::text(c.encode_utf8(&mut [0; 4])))
                        .collect()
                } else {
                    s.split(separator).map(Value::text).collect()
                };
                self.tick(parts.len())?;
                Value::list(parts)
            }
            F::Join => {
                let list = items(args, 0);
                self.tick(list.len())?;
                let separator = text(args, 1);
                let mut out = String::new();
                for (index, item) in list.iter().enumerate() {
                    if index > 0 {
                        out.push_str(separator);
                    }
                    out.push_str(&self.format(item));
                    if out.len() > MAX_TEXT_BYTES {
                        return Ok(Value::Empty);
                    }
                }
                capped_text(out)
            }
            F::Lower => capped_text(s.to_lowercase()),
            F::Upper => capped_text(s.to_uppercase()),
            F::Trim => Value::text(s.trim()),
            F::Repeat => {
                let Some(count) = whole(args, 1) else {
                    return Ok(Value::Empty);
                };
                let count = usize::try_from(count.clamp(0, 10_000)).unwrap_or(0);
                if s.chars().count().saturating_mul(count) > MAX_TEXT_CHARS {
                    Value::Empty
                } else {
                    Value::text(&s.repeat(count))
                }
            }
            F::PadStart | F::PadEnd => pad(args, function == F::PadStart),
            F::Format => capped_text(args.first().map_or_else(String::new, |v| self.format(v))),
            _ => Value::Empty,
        })
    }

    fn replace(&mut self, args: &[Value], all: bool) -> Value {
        let subject = text(args, 0);
        let Some(re) = self.regex(text(args, 1)) else {
            return Value::Empty;
        };
        let replacement = text(args, 2);
        let mut out = String::new();
        let mut last = 0;
        let limit = if all { usize::MAX } else { 1 };
        for captures in regex_js::all_captures(&re, subject, limit) {
            let Some(found) = captures.get(0) else {
                continue;
            };
            out.push_str(subject.get(last..found.start()).unwrap_or_default());
            regex_js::expand(replacement, &captures, &mut out);
            last = found.end();
            if out.len() > MAX_TEXT_BYTES {
                return Value::Empty;
            }
            if !all {
                break;
            }
        }
        out.push_str(subject.get(last..).unwrap_or_default());
        capped_text(out)
    }

    // ---- dates

    fn date_function(&self, function: F, args: &[Value]) -> Value {
        let offset = self.offset_ms();
        match function {
            F::Now => self.local(self.env.now_ms),
            F::Today => match self.env.now_ms.checked_add(offset) {
                Some(ms) => {
                    let midnight = ms.div_euclid(dates::MS_DAY) * dates::MS_DAY;
                    if dates::in_range(midnight) {
                        Value::Date(Date {
                            ms: midnight,
                            time: false,
                        })
                    } else {
                        Value::Empty
                    }
                }
                None => Value::Empty,
            },
            F::Minute | F::Hour | F::Day | F::Date | F::Week | F::Month | F::Year => {
                let Some(moment) = date(args, 0) else {
                    return Value::Empty;
                };
                let p = dates::parts(moment.ms);
                int_value(match function {
                    F::Minute => p.minute,
                    F::Hour => p.hour,
                    F::Day => p.weekday,
                    F::Date => p.day,
                    F::Week => dates::iso_week(&p),
                    F::Month => p.month,
                    _ => p.year,
                })
            }
            F::DateAdd | F::DateSubtract => {
                let (Some(moment), Some(count)) = (date(args, 0), whole(args, 1)) else {
                    return Value::Empty;
                };
                let count = if function == F::DateSubtract {
                    match count.checked_neg() {
                        Some(negated) => negated,
                        None => return Value::Empty,
                    }
                } else {
                    count
                };
                let unit = text(args, 2);
                dates::add(moment.ms, count, unit).map_or(Value::Empty, |ms| {
                    Value::Date(Date {
                        ms,
                        time: moment.time || dates::unit_sets_time(unit),
                    })
                })
            }
            F::DateBetween => match (date(args, 0), date(args, 1)) {
                (Some(a), Some(b)) => {
                    dates::between(a.ms, b.ms, text(args, 2)).map_or(Value::Empty, int_value)
                }
                _ => Value::Empty,
            },
            F::FormatDate => date(args, 0).map_or(Value::Empty, |moment| {
                capped_text(dates::format_date(moment.ms, text(args, 1)))
            }),
            F::ParseDate => dates::parse_date(text(args, 0))
                .map_or(Value::Empty, |(ms, time)| Value::Date(Date { ms, time })),
            F::Timestamp => {
                date(args, 0).map_or(Value::Empty, |moment| int_value(moment.ms - offset))
            }
            F::FromTimestamp => whole(args, 0)
                .and_then(|unix| unix.checked_add(offset))
                .filter(|ms| dates::in_range(*ms))
                .map_or(Value::Empty, |ms| Value::Date(Date { ms, time: true })),
            _ => Value::Empty,
        }
    }

    // ---- lists

    fn list_builtin(&mut self, function: F, args: &[Value]) -> Ran<Value> {
        let list = items(args, 0);
        Ok(match function {
            F::At => whole(args, 1).map_or(Value::Empty, |index| pick(list, index)),
            F::First => list.first().cloned().unwrap_or(Value::Empty),
            F::Last => list.last().cloned().unwrap_or(Value::Empty),
            F::Slice => {
                let len = i64::try_from(list.len()).unwrap_or(i64::MAX);
                let relative = |x: i64| if x < 0 { (len + x).max(0) } else { x.min(len) };
                let Some(start) = whole(args, 1) else {
                    return Ok(Value::Empty);
                };
                let end = if args.len() > 2 {
                    match whole(args, 2) {
                        Some(end) => end,
                        None => return Ok(Value::Empty),
                    }
                } else {
                    len
                };
                let from = usize::try_from(relative(start)).unwrap_or(0);
                let to = usize::try_from(relative(end)).unwrap_or(0);
                match list.get(from..to) {
                    Some(part) if to > from => Value::list(part.to_vec()),
                    _ => Value::list(Vec::new()),
                }
            }
            F::Concat => {
                let mut all: Vec<Value> = Vec::new();
                for arg in args {
                    all.extend(arg.items().iter().cloned());
                    if all.len() > MAX_FORMULA_LIST_ITEMS {
                        return Ok(Value::Empty);
                    }
                }
                self.tick(all.len())?;
                Value::list(all)
            }
            F::Sort => {
                self.tick(list.len())?;
                self.sort(list)
            }
            F::Reverse => {
                self.tick(list.len())?;
                Value::list(list.iter().rev().cloned().collect())
            }
            F::Unique => {
                self.tick(list.len())?;
                let mut seen: HashSet<Identity> = HashSet::new();
                let kept = list
                    .iter()
                    .enumerate()
                    .filter(|(position, item)| seen.insert(identity(item, *position)))
                    .map(|(_, item)| item.clone())
                    .collect();
                Value::list(kept)
            }
            F::Includes => {
                self.tick(list.len())?;
                let target = args.get(1).cloned().unwrap_or(Value::Empty);
                Value::Bool(list.iter().any(|item| equal(item, &target)))
            }
            // The identity once checked: a list never holds a list.
            _ => args.first().cloned().unwrap_or(Value::Empty),
        })
    }

    fn sort_key(&self, value: &Value) -> SortKey {
        match value {
            Value::Number(number) => SortKey::Number(*number),
            Value::Text(text) => SortKey::Text(Arc::clone(text)),
            Value::Bool(flag) => SortKey::Bool(*flag),
            Value::Date(date) => SortKey::Date(date.ms),
            Value::Page(page) => SortKey::Named(
                self.host.title(page.id).unwrap_or_default(),
                page.id.to_string(),
            ),
            Value::Person(account) => SortKey::Named(
                self.host
                    .person(*account)
                    .map(|(name, _)| name)
                    .unwrap_or_default(),
                account.to_string(),
            ),
            Value::Empty | Value::List(_) => SortKey::Empty,
        }
    }

    /// Ascending, stable; empty items last.
    fn sort(&self, list: &[Value]) -> Value {
        let mut keyed: Vec<(SortKey, Value)> = list
            .iter()
            .map(|item| (self.sort_key(item), item.clone()))
            .collect();
        keyed.sort_by(|a, b| compare_keys(&a.0, &b.0));
        Value::list(keyed.into_iter().map(|(_, item)| item).collect())
    }
}

/// `round` (section 3.3): half rounds up, `floor(x + 0.5)`; with places, `round(x * 10^p) / 10^p`
/// and `p` truncated into -10..10.
fn round(args: &[Value]) -> Value {
    let Some(x) = num(args, 0) else {
        return Value::Empty;
    };
    if args.len() < 2 {
        return Value::number((x + 0.5).floor());
    }
    let Some(places) = whole(args, 1) else {
        return Value::Empty;
    };
    let places = i32::try_from(places.clamp(-10, 10)).unwrap_or(0);
    let factor = if places >= 0 {
        10_f64.powi(places)
    } else {
        1.0 / 10_f64.powi(-places)
    };
    Value::number((x * factor + 0.5).floor() / factor)
}

/// `padStart` and `padEnd`: to a length in characters with the pad repeated and cut.
fn pad(args: &[Value], at_start: bool) -> Value {
    let s = text(args, 0);
    let Some(target) = whole(args, 1) else {
        return Value::Empty;
    };
    let Ok(target) = usize::try_from(target.max(0)) else {
        return Value::Empty;
    };
    if target > MAX_TEXT_CHARS {
        return Value::Empty;
    }
    let filler = text(args, 2);
    let len = s.chars().count();
    if target <= len || filler.is_empty() {
        return Value::text(s);
    }
    let fill: String = filler.chars().cycle().take(target - len).collect();
    capped_text(if at_start {
        format!("{fill}{s}")
    } else {
        format!("{s}{fill}")
    })
}
