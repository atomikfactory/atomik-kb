//! The evaluator (`formulas.md` section 4): runs the typed tree of the checker. Empty propagates
//! through arithmetic and dates, an operation that would make a number that is not finite is empty,
//! the budget of steps is shared by every formula that is reached, and the result is normalised as
//! section 4.4 says.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use kb_types::properties::{MAX_FORMULA_LIST_ITEMS, MAX_FORMULA_STEPS, MAX_TEXT_CHARS};
use kb_types::{FormulaResult, OptionId, PageId, PropertyKind, PropertyValue, RollupResult};
use regex::Regex;

use super::ast::CmpOp;
use super::check::{Compiled, MAX_NODE_DEPTH};
use super::dates;
use super::functions::F;
use super::types::{Base, Kind, Node, PropRead, ReadKind, Step, Ty};
use super::value::{Date, PageRef, Value, equal};
use super::{FormulaEnv, FormulaHost};

/// The evaluation stops: the budget is spent or the stack is too deep. The whole formula is empty.
pub(super) struct Abort;

pub(super) type Ran<T> = Result<T, Abort>;

pub(super) struct Eval<'a> {
    pub host: &'a dyn FormulaHost,
    pub env: &'a FormulaEnv,
    nested: &'a HashMap<String, Arc<Node>>,
    steps: usize,
    depth: usize,
    pub scope: Vec<Value>,
    /// The page being computed and how far it is from the page the top formula runs on.
    pub this: PageId,
    level: u8,
    /// The (page, formula property) pairs being evaluated.
    stack: Vec<(PageId, String)>,
    /// Compiled patterns, once per pattern text.
    pub regexes: HashMap<String, Option<Rc<Regex>>>,
}

/// Runs a checked expression on a page; `None` when the result is empty (section 4.4).
pub(super) fn run(
    compiled: &Compiled,
    page: PageId,
    host: &dyn FormulaHost,
    env: &FormulaEnv,
) -> Option<FormulaResult> {
    let mut eval = Eval {
        host,
        env,
        nested: &compiled.nested,
        steps: 0,
        depth: 0,
        scope: Vec::new(),
        this: page,
        level: 0,
        stack: Vec::new(),
        regexes: HashMap::new(),
    };
    let value = eval.node(&compiled.root).ok()?;
    to_result(&normalise(value))
}

/// A count as a number (exact below 2^53).
#[allow(clippy::cast_precision_loss)]
pub(super) fn count_value(count: usize) -> f64 {
    count as f64
}

/// A text of at most `MAX_TEXT_CHARS` characters, else empty.
pub(super) fn capped_text(text: String) -> Value {
    if text.len() > MAX_TEXT_CHARS && text.chars().count() > MAX_TEXT_CHARS {
        Value::Empty
    } else {
        Value::text(&text)
    }
}

impl Eval<'_> {
    pub fn tick(&mut self, steps: usize) -> Ran<()> {
        self.steps = self.steps.saturating_add(steps);
        if self.steps > MAX_FORMULA_STEPS {
            Err(Abort)
        } else {
            Ok(())
        }
    }

    pub fn node(&mut self, node: &Node) -> Ran<Value> {
        self.tick(1)?;
        self.depth += 1;
        if self.depth > MAX_NODE_DEPTH {
            return Err(Abort);
        }
        let value = self.node_inner(node);
        self.depth -= 1;
        value
    }

    /// The dispatcher: every kind of node has a small method of its own, so a deep expression does
    /// not pay for the largest of them on every level of the stack.
    fn node_inner(&mut self, node: &Node) -> Ran<Value> {
        match &node.kind {
            Kind::Num(value) => Ok(Value::Number(*value)),
            Kind::Text(text) => Ok(Value::Text(Arc::clone(text))),
            Kind::Bool(value) => Ok(Value::Bool(*value)),
            Kind::List(items) => self.list_literal(items),
            Kind::Slot(slot) => Ok(self.scope.get(*slot).cloned().unwrap_or(Value::Empty)),
            Kind::Prop(read) => self.this_property(read),
            Kind::PageProp(page, read) => self.page_property(page, read),
            Kind::Neg(inner) => self.negate(inner),
            Kind::Not(inner) => Ok(Value::Bool(!self.node(inner)?.truthy())),
            Kind::And(operands) => self.all_of(operands),
            Kind::Or(operands) => self.any_of(operands),
            Kind::Cmp(op, left, right) => self.comparison(*op, left, right),
            Kind::Chain(first, steps) => self.chain(first, steps),
            Kind::Pow(base, exponent) => self.power(base, exponent),
            Kind::Cond(arms, other) => self.conditional(arms, other),
            Kind::Lets(values, body) => self.lets(values, body),
            Kind::Call(function, args) => self.call_node(*function, args),
            Kind::ListFn(function, list, body) => {
                self.list_function(*function, list, body.as_deref())
            }
        }
    }

    fn list_literal(&mut self, items: &[Node]) -> Ran<Value> {
        let mut values = Vec::with_capacity(items.len());
        for item in items {
            values.push(self.node(item)?);
        }
        Ok(Value::list(values))
    }

    fn this_property(&mut self, read: &PropRead) -> Ran<Value> {
        let (page, level) = (self.this, self.level);
        self.read(page, level, read)
    }

    /// Only a page reached through one relation of this page can be read.
    fn page_property(&mut self, page: &Node, read: &PropRead) -> Ran<Value> {
        match self.node(page)? {
            Value::Page(PageRef { id, level: 1 }) => self.read(id, 1, read),
            _ => Ok(Value::Empty),
        }
    }

    fn negate(&mut self, inner: &Node) -> Ran<Value> {
        Ok(match self.node(inner)? {
            Value::Number(value) => Value::Number(-value),
            _ => Value::Empty,
        })
    }

    fn all_of(&mut self, operands: &[Node]) -> Ran<Value> {
        for operand in operands {
            if !self.node(operand)?.truthy() {
                return Ok(Value::Bool(false));
            }
        }
        Ok(Value::Bool(true))
    }

    fn any_of(&mut self, operands: &[Node]) -> Ran<Value> {
        for operand in operands {
            if self.node(operand)?.truthy() {
                return Ok(Value::Bool(true));
            }
        }
        Ok(Value::Bool(false))
    }

    fn comparison(&mut self, op: CmpOp, left: &Node, right: &Node) -> Ran<Value> {
        let l = self.node(left)?;
        let r = self.node(right)?;
        Ok(Value::Bool(compare(op, &l, &r)))
    }

    fn chain(&mut self, first: &Node, steps: &[(Step, Node)]) -> Ran<Value> {
        let mut acc = self.node(first)?;
        for (step, operand) in steps {
            let rhs = self.node(operand)?;
            acc = self.step(*step, &acc, &rhs);
        }
        Ok(acc)
    }

    fn power(&mut self, base: &Node, exponent: &Node) -> Ran<Value> {
        let b = self.node(base)?;
        let e = self.node(exponent)?;
        Ok(match (b, e) {
            (Value::Number(b), Value::Number(e)) => Value::number(b.powf(e)),
            _ => Value::Empty,
        })
    }

    fn conditional(&mut self, arms: &[(Node, Node)], other: &Node) -> Ran<Value> {
        for (cond, value) in arms {
            if self.node(cond)?.truthy() {
                return self.node(value);
            }
        }
        self.node(other)
    }

    fn lets(&mut self, values: &[Node], body: &Node) -> Ran<Value> {
        let base = self.scope.len();
        for value in values {
            let bound = self.node(value)?;
            self.scope.push(bound);
        }
        let result = self.node(body);
        self.scope.truncate(base);
        result
    }

    fn call_node(&mut self, function: F, args: &[Node]) -> Ran<Value> {
        let mut values = Vec::with_capacity(args.len());
        for arg in args {
            values.push(self.node(arg)?);
        }
        self.call(function, &values)
    }

    /// One step of an arithmetic chain.
    fn step(&self, step: Step, left: &Value, right: &Value) -> Value {
        if step == Step::Concat {
            let mut text = self.format(left);
            text.push_str(&self.format(right));
            return capped_text(text);
        }
        let (Value::Number(a), Value::Number(b)) = (left, right) else {
            return Value::Empty;
        };
        Value::number(match step {
            Step::Add => a + b,
            Step::Sub => a - b,
            Step::Mul => a * b,
            Step::Div => a / b,
            Step::Rem => a % b,
            Step::Concat => return Value::Empty,
        })
    }

    /// `map`, `filter`, `find`, `findIndex`, `some`, `every`, `count`.
    fn list_function(&mut self, function: F, list: &Node, body: Option<&Node>) -> Ran<Value> {
        let source = self.node(list)?;
        let items = source.items();
        let Some(body) = body else {
            // `count` of a list without a condition.
            return Ok(Value::Number(count_value(items.len())));
        };
        let base = self.scope.len();
        self.scope.push(Value::Empty);
        self.scope.push(Value::Empty);
        let mut kept: Vec<Value> = Vec::new();
        let mut counted = 0_usize;
        let mut found: Option<(usize, Value)> = None;
        let mut all = true;
        for (index, item) in items.iter().enumerate() {
            self.tick(1)?;
            if let Some(slot) = self.scope.get_mut(base) {
                *slot = item.clone();
            }
            if let Some(slot) = self.scope.get_mut(base + 1) {
                *slot = Value::Number(count_value(index));
            }
            let result = self.node(body)?;
            match function {
                F::Map => kept.push(result),
                F::Filter => {
                    if result.truthy() {
                        kept.push(item.clone());
                    }
                }
                F::Find | F::FindIndex | F::Any => {
                    if result.truthy() {
                        found = Some((index, item.clone()));
                        break;
                    }
                }
                F::All => {
                    if !result.truthy() {
                        all = false;
                        break;
                    }
                }
                _ => {
                    if result.truthy() {
                        counted += 1;
                    }
                }
            }
        }
        self.scope.truncate(base);
        Ok(match function {
            F::Map | F::Filter => Value::list(kept),
            F::Find => found.map_or(Value::Empty, |(_, item)| item),
            F::FindIndex => Value::Number(found.map_or(-1.0, |(index, _)| count_value(index))),
            F::Any => Value::Bool(found.is_some()),
            F::All => Value::Bool(all),
            _ => Value::Number(count_value(counted)),
        })
    }

    // ---- properties

    /// The local date of a Unix ms (with a time).
    pub fn local(&self, unix_ms: i64) -> Value {
        let offset = i64::from(self.env.offset_seconds) * 1000;
        match unix_ms.checked_add(offset) {
            Some(ms) if dates::in_range(ms) => Value::Date(Date { ms, time: true }),
            _ => Value::Empty,
        }
    }

    /// A property of a page (`level` is the page's distance from the top page).
    fn read(&mut self, page: PageId, level: u8, read: &PropRead) -> Ran<Value> {
        let kind = match read.kind {
            ReadKind::Title => {
                let title = self.host.title(page).unwrap_or_default();
                return Ok(capped_text(title));
            }
            ReadKind::Kind(kind) => kind,
        };
        match kind {
            PropertyKind::Relation => {
                let related = match read.pid {
                    Some(pid) => self.host.related(page, pid),
                    None => Vec::new(),
                };
                self.tick(related.len())?;
                let items = related
                    .into_iter()
                    .map(|id| {
                        Value::Page(PageRef {
                            id,
                            level: level.saturating_add(1),
                        })
                    })
                    .collect();
                Ok(Value::list(items))
            }
            PropertyKind::Formula => self.formula(page, level, read),
            PropertyKind::CreatedTime => Ok(self
                .host
                .created_ms(page)
                .map_or(Value::Empty, |ms| self.local(ms))),
            PropertyKind::LastEditedTime => Ok(self
                .host
                .edited_ms(page)
                .map_or(Value::Empty, |ms| self.local(ms))),
            _ => {
                let stored = read.pid.and_then(|pid| self.host.value(page, pid));
                Ok(self.stored(read, kind, stored))
            }
        }
    }

    /// The value of another formula property on a page: what the page's values show (section 4.4).
    fn formula(&mut self, page: PageId, level: u8, read: &PropRead) -> Ran<Value> {
        let default = |ty: Ty| settle(Value::Empty, ty);
        let Some(node) = self.nested.get(&read.id).map(Arc::clone) else {
            return Ok(default(read.ty));
        };
        if self
            .stack
            .iter()
            .any(|(p, id)| *p == page && *id == read.id)
        {
            return Ok(default(read.ty));
        }
        self.stack.push((page, read.id.clone()));
        let saved_scope = std::mem::take(&mut self.scope);
        let saved_this = std::mem::replace(&mut self.this, page);
        let saved_level = std::mem::replace(&mut self.level, level);
        let outcome = self.node(&node);
        self.level = saved_level;
        self.this = saved_this;
        self.scope = saved_scope;
        self.stack.pop();
        Ok(settle(outcome?, node.ty))
    }

    fn option_name(options: &[(OptionId, String)], id: OptionId) -> Option<Value> {
        options
            .iter()
            .find(|(option, _)| *option == id)
            .map(|(_, name)| Value::text(name))
    }

    /// The value a stored property gives (the table of section 2.3).
    fn stored(&self, read: &PropRead, kind: PropertyKind, stored: Option<PropertyValue>) -> Value {
        match (kind, stored) {
            (
                PropertyKind::Text | PropertyKind::Url | PropertyKind::Email | PropertyKind::Phone,
                Some(
                    PropertyValue::Text(text)
                    | PropertyValue::Url(text)
                    | PropertyValue::Email(text)
                    | PropertyValue::Phone(text),
                ),
            ) => capped_text(text),
            (PropertyKind::Number, Some(PropertyValue::Number(value))) => Value::number(value),
            (
                PropertyKind::Select | PropertyKind::Status,
                Some(PropertyValue::Select(id) | PropertyValue::Status(id)),
            ) => Self::option_name(&read.options, id).unwrap_or_else(|| Value::text("")),
            (PropertyKind::MultiSelect, Some(PropertyValue::MultiSelect(ids))) => Value::list(
                ids.into_iter()
                    .filter_map(|id| Self::option_name(&read.options, id))
                    .collect(),
            ),
            (PropertyKind::Date, Some(PropertyValue::Date(date))) => dates::from_value(&date)
                .map_or(Value::Empty, |(ms, time)| Value::Date(Date { ms, time })),
            (PropertyKind::Checkbox, Some(PropertyValue::Checkbox(value))) => Value::Bool(value),
            (PropertyKind::Files, Some(PropertyValue::Files(ids))) => Value::list(
                ids.into_iter()
                    .filter_map(|id| self.host.file_name(id))
                    .map(|name| Value::text(&name))
                    .collect(),
            ),
            (PropertyKind::Checklist, Some(PropertyValue::Checklist(items))) => Value::list(
                items
                    .into_iter()
                    .map(|item| Value::Bool(item.done))
                    .collect(),
            ),
            (PropertyKind::Place, Some(PropertyValue::Place(place))) => capped_text(place.name),
            (PropertyKind::Person, Some(PropertyValue::Person(ids))) => {
                Value::list(ids.into_iter().map(Value::Person).collect())
            }
            (PropertyKind::Rollup, Some(PropertyValue::Rollup(rollup))) => {
                self.rollup(read, rollup.result)
            }
            // An unset property, or a value of another kind than the schema's.
            (_, _) => unset(read.ty),
        }
    }

    fn rollup(&self, read: &PropRead, result: RollupResult) -> Value {
        match result {
            RollupResult::Number(value) => Value::number(value),
            RollupResult::Date(date) => dates::from_value(&date)
                .map_or(Value::Empty, |(ms, time)| Value::Date(Date { ms, time })),
            RollupResult::Progress { done, total } => {
                if total == 0 {
                    Value::Empty
                } else {
                    Value::number(f64::from(done) / f64::from(total))
                }
            }
            RollupResult::Values(values) => {
                let mut items = Vec::new();
                for value in values {
                    self.flatten_into(read, value, &mut items);
                }
                Value::list(items)
            }
        }
    }

    /// The items a rolled-up value contributes (a list value is flattened).
    fn flatten_into(&self, read: &PropRead, value: PropertyValue, items: &mut Vec<Value>) {
        match value {
            PropertyValue::Text(text)
            | PropertyValue::Url(text)
            | PropertyValue::Email(text)
            | PropertyValue::Phone(text) => items.push(capped_text(text)),
            PropertyValue::Number(value) => items.push(Value::number(value)),
            PropertyValue::Select(id) | PropertyValue::Status(id) => {
                items.extend(Self::option_name(&read.options, id));
            }
            PropertyValue::MultiSelect(ids) => {
                items.extend(
                    ids.into_iter()
                        .filter_map(|id| Self::option_name(&read.options, id)),
                );
            }
            PropertyValue::Date(date) => {
                if let Some((ms, time)) = dates::from_value(&date) {
                    items.push(Value::Date(Date { ms, time }));
                }
            }
            PropertyValue::Checkbox(value) => items.push(Value::Bool(value)),
            PropertyValue::Checklist(list) => {
                items.extend(list.into_iter().map(|item| Value::Bool(item.done)));
            }
            PropertyValue::Place(place) => items.push(capped_text(place.name)),
            PropertyValue::Person(ids) => items.extend(ids.into_iter().map(Value::Person)),
            PropertyValue::Files(ids) => {
                items.extend(
                    ids.into_iter()
                        .filter_map(|id| self.host.file_name(id))
                        .map(|name| Value::text(&name)),
                );
            }
            _ => {}
        }
    }
}

/// What an unset property reads as: a list kind `[]`, a text-like kind `""`, a checkbox `false`,
/// the rest empty.
fn unset(ty: Ty) -> Value {
    if ty.list {
        Value::List(Rc::new(Vec::new()))
    } else if ty.is_known(Base::Text) {
        Value::text("")
    } else if ty.is_known(Base::Boolean) {
        Value::Bool(false)
    } else {
        Value::Empty
    }
}

/// A nested formula's value as another formula reads it: the normalised result, and what an
/// unset property of its type reads as when there is none.
fn settle(value: Value, ty: Ty) -> Value {
    match normalise(value) {
        Value::Empty => {
            if ty.list {
                Value::List(Rc::new(Vec::new()))
            } else if ty.is_known(Base::Text) {
                Value::text("")
            } else {
                Value::Empty
            }
        }
        value => value,
    }
}

/// `<`, `<=`, `>`, `>=`, `==`, `!=` (section 3.2): a comparison with an empty side is false (but
/// `!=` is the opposite of `==`).
fn compare(op: CmpOp, left: &Value, right: &Value) -> bool {
    match op {
        CmpOp::Eq => equal(left, right),
        CmpOp::Ne => !equal(left, right),
        _ => ordering(left, right).is_some_and(|order| match op {
            CmpOp::Lt => order == Ordering::Less,
            CmpOp::Le => order != Ordering::Greater,
            CmpOp::Gt => order == Ordering::Greater,
            _ => order != Ordering::Less,
        }),
    }
}

fn ordering(left: &Value, right: &Value) -> Option<Ordering> {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => a.partial_cmp(b),
        (Value::Text(a), Value::Text(b)) => Some(a.as_ref().cmp(b.as_ref())),
        (Value::Date(a), Value::Date(b)) => Some(a.ms.cmp(&b.ms)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        _ => None,
    }
}

/// Section 4.4: a text that is empty becomes nothing, a list loses its empty items and becomes
/// nothing when none is left (or when it is too long); `-0` is `0`.
pub(super) fn normalise(value: Value) -> Value {
    match value {
        Value::Number(0.0) => Value::Number(0.0),
        Value::Number(number) => Value::number(number),
        Value::Text(text) => {
            if text.is_empty()
                || (text.len() > MAX_TEXT_CHARS && text.chars().count() > MAX_TEXT_CHARS)
            {
                Value::Empty
            } else {
                Value::Text(text)
            }
        }
        Value::Date(date) => {
            if dates::in_range(date.ms) {
                Value::Date(date)
            } else {
                Value::Empty
            }
        }
        Value::List(items) => {
            let kept: Vec<Value> = items
                .iter()
                .cloned()
                .map(normalise)
                .filter(|item| !matches!(item, Value::Empty))
                .collect();
            if kept.is_empty() || kept.len() > MAX_FORMULA_LIST_ITEMS {
                Value::Empty
            } else {
                Value::List(Rc::new(kept))
            }
        }
        other => other,
    }
}

/// A normalised value as the result of the formula.
fn to_result(value: &Value) -> Option<FormulaResult> {
    match value {
        Value::Empty => None,
        Value::Number(number) => Some(FormulaResult::Number(*number)),
        Value::Text(text) => Some(FormulaResult::Text(text.to_string())),
        Value::Bool(flag) => Some(FormulaResult::Boolean(*flag)),
        Value::Date(date) => dates::to_value(date.ms, date.time).map(FormulaResult::Date),
        Value::Page(page) => Some(FormulaResult::Page(page.id)),
        Value::Person(account) => Some(FormulaResult::Person(*account)),
        Value::List(items) => items
            .iter()
            .map(to_result)
            .collect::<Option<Vec<_>>>()
            .map(FormulaResult::List),
    }
}
