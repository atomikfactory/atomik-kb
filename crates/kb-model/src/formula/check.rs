//! The static checker (`formulas.md` sections 2.3, 3 and 4.3): the strict types of every operator
//! and function, the scopes of `let` and of the list functions, the properties against the schema
//! and the cycles among formulas. It builds the typed tree the evaluator runs ([`Node`]), so a
//! check and an evaluation agree on every type (an empty text and an empty number behave
//! differently).

use std::collections::HashMap;
use std::sync::Arc;

use kb_types::properties::FORMULA_TITLE_ID;
use kb_types::{PropertyId, PropertyKind, RollupFunction};

use super::ast::{ArithOp, Call, CmpOp, Expr, ExprKind};
use super::functions::{self, F, Info};
use super::parser;
use super::types::{Base, Kind, Node, PropRead, ReadKind, Step, Ty};
use super::{FormulaError, FormulaErrorCode, FormulaProperty, FormulaSchema};

/// The deepest tree the checker and the evaluator follow, and so the deepest recursion. The parser
/// bounds the nesting of parentheses, calls and lists; this bounds what it does not (a long chain of
/// method calls, a long chain of formulas that read one another) and keeps the stack safe.
pub(super) const MAX_NODE_DEPTH: usize = 320;

/// Errors are boxed: the checker recurses deeply and a `Result` of the size of an error on every
/// frame is what makes a deep expression use a lot of stack.
type Checked<T> = Result<T, Box<FormulaError>>;

/// A checked expression and the formulas of the schema it reaches.
pub(super) struct Compiled {
    pub root: Node,
    /// The checked trees of the formula properties reached through `prop` and `.prop`, by id.
    pub nested: HashMap<String, Arc<Node>>,
}

/// Checks a parsed expression against the schema.
pub(super) fn compile(
    ast: &Expr,
    schema: &dyn FormulaSchema,
    self_id: Option<&str>,
) -> Result<Compiled, FormulaError> {
    let mut checker = Checker {
        schema,
        self_id,
        scope: Vec::new(),
        visiting: Vec::new(),
        done: HashMap::new(),
        props: HashMap::new(),
        depth: 0,
    };
    checker.prepare(ast);
    let root = checker.expr(ast).map_err(|error| *error)?;
    let nested = checker
        .done
        .into_iter()
        .filter_map(|(id, outcome)| outcome.ok().map(|node| (id, node)))
        .collect();
    Ok(Compiled { root, nested })
}

struct Checker<'a> {
    schema: &'a dyn FormulaSchema,
    self_id: Option<&'a str>,
    /// The names in scope (let names, `current`, `index`) and their types; the position is the slot.
    scope: Vec<(String, Ty)>,
    /// The formula properties being checked, outermost first.
    visiting: Vec<String>,
    /// The formula properties checked so far.
    done: HashMap<String, Checked<Arc<Node>>>,
    props: HashMap<String, Option<FormulaProperty>>,
    depth: usize,
}

fn error(
    code: FormulaErrorCode,
    start: usize,
    end: usize,
    message: String,
    detail: String,
) -> Box<FormulaError> {
    Box::new(FormulaError::new(code, message, start, end, detail))
}

fn type_error(start: usize, end: usize, expected: Ty, found: Ty) -> Box<FormulaError> {
    let (expected, found) = (expected.name(), found.name());
    error(
        FormulaErrorCode::Type,
        start,
        end,
        format!("expected {expected}, found {found}"),
        format!("{expected}:{found}"),
    )
}

fn type_mismatch(at: &Expr, expected: Ty, found: Ty) -> Box<FormulaError> {
    type_error(at.start, at.end, expected, found)
}

fn arity_error(call: &Expr, name: &str, info: Info) -> Box<FormulaError> {
    let max = info
        .max
        .map_or(-1, |max| i64::try_from(max).unwrap_or(i64::MAX));
    let wanted = match info.max {
        Some(max) if max == info.min => format!("{max}"),
        Some(max) => format!("{} to {max}", info.min),
        None => format!("at least {}", info.min),
    };
    error(
        FormulaErrorCode::Arity,
        call.start,
        call.end,
        format!("{name} takes {wanted} arguments"),
        format!("{name}:{}:{max}", info.min),
    )
}

#[cold]
fn unknown_function(call: &Call) -> Box<FormulaError> {
    error(
        FormulaErrorCode::UnknownFunction,
        call.name_start,
        call.name_end,
        format!("unknown function \"{}\"", call.name),
        call.name.clone(),
    )
}

#[cold]
fn unknown_function_at(call: &Expr) -> Box<FormulaError> {
    error(
        FormulaErrorCode::UnknownFunction,
        call.start,
        call.end,
        "unknown function".to_owned(),
        String::new(),
    )
}

fn cycle_error(start: usize, end: usize, name: &str) -> Box<FormulaError> {
    error(
        FormulaErrorCode::Cycle,
        start,
        end,
        format!("\"{name}\" depends on this formula"),
        name.to_owned(),
    )
}

/// The error a reference to a formula that does not check is: a loop stays a loop; anything else
/// makes the property unusable in a formula.
fn nested_error(at: &Expr, property: &FormulaProperty, inner: &FormulaError) -> Box<FormulaError> {
    if inner.code == FormulaErrorCode::Cycle {
        return cycle_error(at.start, at.end, &inner.detail);
    }
    // A wrapper of a wrapper only says which formula it is (the messages would grow with the chain).
    let message = if inner.code == FormulaErrorCode::UnsupportedProperty {
        format!("the formula of \"{}\" has an error", property.name)
    } else {
        format!("the formula of \"{}\" has an error: {inner}", property.name)
    };
    error(
        FormulaErrorCode::UnsupportedProperty,
        at.start,
        at.end,
        message,
        property.name.clone(),
    )
}

/// What a fixed-signature parameter accepts.
#[derive(Clone, Copy)]
enum P {
    Num,
    Text,
    Bool,
    Date,
    Page,
    Person,
    /// Any list.
    List,
    /// Anything, a list included.
    Any,
    /// A text or a list.
    TextOrList,
    /// A text, boolean, date or number.
    Convertible,
}

/// What a fixed-signature function returns.
#[derive(Clone, Copy)]
enum Res {
    Fixed(Ty),
    /// The type of the first argument.
    Arg0,
    /// The item type of the first argument (a list).
    Item0,
}

struct Sig {
    required: &'static [P],
    optional: &'static [P],
    result: Res,
}

const fn sig(required: &'static [P], optional: &'static [P], result: Res) -> Option<Sig> {
    Some(Sig {
        required,
        optional,
        result,
    })
}

fn signature(f: F) -> Option<Sig> {
    use P::{Any, Bool, Convertible, Date, List, Num, Page, Person, Text, TextOrList};
    const NUMBER: Res = Res::Fixed(Ty::NUMBER);
    const TEXT: Res = Res::Fixed(Ty::TEXT);
    const BOOLEAN: Res = Res::Fixed(Ty::BOOLEAN);
    const DATE: Res = Res::Fixed(Ty::DATE);
    match f {
        F::Not => sig(&[Bool], &[], BOOLEAN),
        F::Empty => sig(&[Any], &[], BOOLEAN),
        F::Add | F::Subtract | F::Multiply | F::Divide | F::Mod | F::Pow => {
            sig(&[Num, Num], &[], NUMBER)
        }
        F::Abs
        | F::Ceil
        | F::Floor
        | F::Sqrt
        | F::Cbrt
        | F::Exp
        | F::Ln
        | F::Log10
        | F::Log2
        | F::Sign => sig(&[Num], &[], NUMBER),
        F::Pi | F::E => sig(&[], &[], NUMBER),
        F::Round => sig(&[Num], &[Num], NUMBER),
        F::ToNumber => sig(&[Convertible], &[], NUMBER),
        F::Length => sig(&[TextOrList], &[], NUMBER),
        F::Substring => sig(&[Text, Num], &[Num], TEXT),
        F::Contains | F::Test => sig(&[Text, Text], &[], BOOLEAN),
        F::Match => sig(&[Text, Text], &[], Res::Fixed(Ty::list_of(Base::Text))),
        F::Replace | F::ReplaceAll => sig(&[Text, Text, Text], &[], TEXT),
        F::Split => sig(&[Text, Text], &[], Res::Fixed(Ty::list_of(Base::Text))),
        F::Join => sig(&[List, Text], &[], TEXT),
        F::Lower | F::Upper | F::Trim => sig(&[Text], &[], TEXT),
        F::Repeat => sig(&[Text, Num], &[], TEXT),
        F::PadStart | F::PadEnd => sig(&[Text, Num, Text], &[], TEXT),
        F::Format => sig(&[Any], &[], TEXT),
        F::Now | F::Today => sig(&[], &[], DATE),
        F::Minute | F::Hour | F::Day | F::Date | F::Week | F::Month | F::Year => {
            sig(&[Date], &[], NUMBER)
        }
        F::DateAdd | F::DateSubtract => sig(&[Date, Num, Text], &[], DATE),
        F::DateBetween => sig(&[Date, Date, Text], &[], NUMBER),
        F::FormatDate => sig(&[Date, Text], &[], TEXT),
        F::ParseDate => sig(&[Text], &[], DATE),
        F::Timestamp => sig(&[Date], &[], NUMBER),
        F::FromTimestamp => sig(&[Num], &[], DATE),
        F::At => sig(&[List, Num], &[], Res::Item0),
        F::First | F::Last => sig(&[List], &[], Res::Item0),
        F::Slice => sig(&[List, Num], &[Num], Res::Arg0),
        F::Sort | F::Reverse | F::Unique | F::Flat => sig(&[List], &[], Res::Arg0),
        F::Id => sig(&[], &[Page], TEXT),
        F::Name | F::Email => sig(&[Person], &[], TEXT),
        _ => None,
    }
}

fn literal(kind: Kind, ty: Ty) -> Node {
    Node { kind, ty }
}

fn expect_param(e: &Expr, node: Node, kind: P) -> Checked<Node> {
    match kind {
        P::Num => expect_scalar(e, node, Base::Number),
        P::Text => expect_scalar(e, node, Base::Text),
        P::Bool => expect_scalar(e, node, Base::Boolean),
        P::Date => expect_scalar(e, node, Base::Date),
        P::Page => expect_scalar(e, node, Base::Page),
        P::Person => expect_scalar(e, node, Base::Person),
        P::List => expect_list(e, node),
        P::Any => Ok(node),
        P::TextOrList => {
            if node.ty.list || node.ty.is(Base::Text) {
                Ok(node)
            } else {
                Err(type_mismatch(e, Ty::TEXT, node.ty))
            }
        }
        P::Convertible => {
            let fits = !node.ty.list
                && node.ty.base.is_none_or(|base| {
                    matches!(base, Base::Text | Base::Boolean | Base::Date | Base::Number)
                });
            if fits {
                Ok(node)
            } else {
                Err(type_mismatch(e, Ty::TEXT, node.ty))
            }
        }
    }
}

fn expect_scalar(e: &Expr, node: Node, base: Base) -> Checked<Node> {
    if node.ty.is(base) {
        Ok(node)
    } else {
        Err(type_mismatch(e, Ty::of(base), node.ty))
    }
}

fn expect_list(e: &Expr, node: Node) -> Checked<Node> {
    if node.ty.list {
        Ok(node)
    } else {
        Err(type_mismatch(e, Ty::OPEN_LIST, node.ty))
    }
}

impl Checker<'_> {
    fn expr(&mut self, e: &Expr) -> Checked<Node> {
        self.depth += 1;
        if self.depth > MAX_NODE_DEPTH {
            self.depth -= 1;
            return Err(error(
                FormulaErrorCode::TooLong,
                e.start,
                e.end,
                "the formula is nested too deeply".to_owned(),
                String::new(),
            ));
        }
        let checked = self.expr_inner(e);
        self.depth -= 1;
        checked
    }

    fn expr_inner(&mut self, e: &Expr) -> Checked<Node> {
        match &e.kind {
            ExprKind::Number(value) => Ok(literal(Kind::Num(*value), Ty::NUMBER)),
            ExprKind::Str(value) => Ok(literal(Kind::Text(Arc::from(value.as_str())), Ty::TEXT)),
            ExprKind::Bool(value) => Ok(literal(Kind::Bool(*value), Ty::BOOLEAN)),
            ExprKind::List(items) => self.list_literal(items),
            ExprKind::Var(name) => self.variable(e, name),
            ExprKind::Call(call) => self.call(e, call),
            ExprKind::Neg(inner) => self.prefix(inner, Base::Number, Kind::Neg),
            ExprKind::Not(inner) => self.prefix(inner, Base::Boolean, Kind::Not),
            ExprKind::And(operands) => self.logic(operands, Kind::And),
            ExprKind::Or(operands) => self.logic(operands, Kind::Or),
            ExprKind::Compare(op, left, right) => self.compare(*op, left, right),
            ExprKind::Arith(first, rest) => self.arith(first, rest),
            ExprKind::Pow(base, exponent) => self.power(base, exponent),
            ExprKind::Ternary(cond, then, other) => self.ternary(cond, then, other),
        }
    }

    /// `-x` and `not x`.
    fn prefix(&mut self, inner: &Expr, base: Base, wrap: fn(Box<Node>) -> Kind) -> Checked<Node> {
        let operand = self.scalar(inner, base)?;
        Ok(literal(wrap(Box::new(operand)), Ty::of(base)))
    }

    /// `and` and `or`, as operators or functions.
    fn logic(&mut self, operands: &[Expr], wrap: fn(Vec<Node>) -> Kind) -> Checked<Node> {
        let nodes = self.booleans(operands)?;
        Ok(literal(wrap(nodes), Ty::BOOLEAN))
    }

    fn power(&mut self, base: &Expr, exponent: &Expr) -> Checked<Node> {
        let base = self.scalar(base, Base::Number)?;
        let exponent = self.scalar(exponent, Base::Number)?;
        Ok(literal(
            Kind::Pow(Box::new(base), Box::new(exponent)),
            Ty::NUMBER,
        ))
    }

    fn ternary(&mut self, cond: &Expr, then: &Expr, other: &Expr) -> Checked<Node> {
        let cond = self.scalar(cond, Base::Boolean)?;
        let then_node = self.expr(then)?;
        let other_node = self.expr(other)?;
        let ty = then_node
            .ty
            .unify(other_node.ty)
            .ok_or_else(|| type_mismatch(other, then_node.ty, other_node.ty))?;
        Ok(literal(
            Kind::Cond(vec![(cond, then_node)], Box::new(other_node)),
            ty,
        ))
    }

    // ---- typed arguments

    /// An expression that must be a scalar of this base (an open scalar passes).
    fn scalar(&mut self, e: &Expr, base: Base) -> Checked<Node> {
        let node = self.expr(e)?;
        expect_scalar(e, node, base)
    }

    fn any_list(&mut self, e: &Expr) -> Checked<Node> {
        let node = self.expr(e)?;
        expect_list(e, node)
    }

    fn booleans(&mut self, operands: &[Expr]) -> Checked<Vec<Node>> {
        let mut nodes = Vec::with_capacity(operands.len());
        for operand in operands {
            nodes.push(self.scalar(operand, Base::Boolean)?);
        }
        Ok(nodes)
    }

    /// An argument of a fixed signature: checked first, then held to the parameter's kind (after
    /// the recursion, so that nothing of the kind check stays on the stack for the whole argument).
    fn param(&mut self, e: &Expr, kind: P) -> Checked<Node> {
        let node = self.expr(e)?;
        expect_param(e, node, kind)
    }

    // ---- literals, names and operators

    fn list_literal(&mut self, items: &[Expr]) -> Checked<Node> {
        let mut nodes = Vec::with_capacity(items.len());
        let mut item_ty: Option<Ty> = None;
        for item in items {
            let node = self.expr(item)?;
            if node.ty.list {
                return Err(type_mismatch(
                    item,
                    item_ty.unwrap_or(node.ty.item()),
                    node.ty,
                ));
            }
            item_ty = Some(match item_ty {
                None => node.ty,
                Some(seen) => seen
                    .unify(node.ty)
                    .ok_or_else(|| type_mismatch(item, seen, node.ty))?,
            });
            nodes.push(node);
        }
        let ty = Ty {
            base: item_ty.and_then(|ty| ty.base),
            list: true,
        };
        Ok(Node {
            kind: Kind::List(nodes),
            ty,
        })
    }

    fn variable(&self, e: &Expr, name: &str) -> Checked<Node> {
        match self.scope.iter().rposition(|(bound, _)| bound == name) {
            Some(slot) => Ok(Node {
                kind: Kind::Slot(slot),
                ty: self.scope.get(slot).map_or(Ty::TEXT, |(_, ty)| *ty),
            }),
            None => Err(error(
                FormulaErrorCode::UnknownVariable,
                e.start,
                e.end,
                format!("unknown variable \"{name}\""),
                name.to_owned(),
            )),
        }
    }

    fn compare(&mut self, op: CmpOp, left: &Expr, right: &Expr) -> Checked<Node> {
        let l = self.expr(left)?;
        let r = self.expr(right)?;
        if !matches!(op, CmpOp::Eq | CmpOp::Ne) {
            let ordered = |ty: Ty| {
                !ty.list
                    && ty.base.is_none_or(|base| {
                        matches!(base, Base::Number | Base::Text | Base::Date | Base::Boolean)
                    })
            };
            if !ordered(l.ty) {
                return Err(type_mismatch(left, Ty::NUMBER, l.ty));
            }
            if !ordered(r.ty) {
                return Err(type_mismatch(right, Ty::NUMBER, r.ty));
            }
        }
        if l.ty.unify(r.ty).is_none() {
            return Err(type_mismatch(right, l.ty, r.ty));
        }
        Ok(Node {
            kind: Kind::Cmp(op, Box::new(l), Box::new(r)),
            ty: Ty::BOOLEAN,
        })
    }

    fn arith(&mut self, first: &Expr, rest: &[(ArithOp, Expr)]) -> Checked<Node> {
        let head = self.expr(first)?;
        let mut acc_ty = head.ty;
        let acc_start = first.start;
        let mut acc_end = first.end;
        let mut steps = Vec::with_capacity(rest.len());
        for (op, operand) in rest {
            let rhs = self.expr(operand)?;
            let text_side = acc_ty.is_known(Base::Text) || rhs.ty.is_known(Base::Text);
            let step = if *op == ArithOp::Add && text_side {
                acc_ty = Ty::TEXT;
                Step::Concat
            } else {
                if !acc_ty.is(Base::Number) {
                    return Err(type_error(acc_start, acc_end, Ty::NUMBER, acc_ty));
                }
                if !rhs.ty.is(Base::Number) {
                    return Err(type_mismatch(operand, Ty::NUMBER, rhs.ty));
                }
                acc_ty = Ty::NUMBER;
                match op {
                    ArithOp::Add => Step::Add,
                    ArithOp::Sub => Step::Sub,
                    ArithOp::Mul => Step::Mul,
                    ArithOp::Div => Step::Div,
                    ArithOp::Rem => Step::Rem,
                }
            };
            acc_end = operand.end;
            steps.push((step, rhs));
        }
        Ok(Node {
            kind: Kind::Chain(Box::new(head), steps),
            ty: acc_ty,
        })
    }

    // ---- calls

    fn call(&mut self, e: &Expr, call: &Call) -> Checked<Node> {
        let Some(info) = functions::lookup(&call.name) else {
            return Err(unknown_function(call));
        };
        let count = call.args.len();
        let odd_only = matches!(info.f, F::Ifs | F::Lets);
        if count < info.min
            || info.max.is_some_and(|max| count > max)
            || (odd_only && count.is_multiple_of(2))
        {
            return Err(arity_error(e, &call.name, info));
        }
        let args = call.args.as_slice();
        match info.f {
            F::Prop => self.property_call(args),
            F::Let | F::Lets => self.bind(args),
            F::If | F::Ifs => self.conditional(args),
            F::And => self.logic(args, Kind::And),
            F::Or => self.logic(args, Kind::Or),
            F::Map | F::Filter | F::Find | F::FindIndex | F::Any | F::All | F::Count => {
                self.list_function(info.f, args)
            }
            F::Min | F::Max | F::Sum | F::Mean | F::Median => self.numbers(info.f, args),
            F::Concat => self.concat(args),
            F::Includes => self.includes(args),
            F::Equal | F::Unequal => self.equality(info.f, args),
            f => self.fixed(e, f, args),
        }
    }

    /// A function with a fixed signature.
    fn fixed(&mut self, call: &Expr, f: F, args: &[Expr]) -> Checked<Node> {
        let Some(signature) = signature(f) else {
            return Err(unknown_function_at(call));
        };
        let mut nodes = Vec::with_capacity(args.len());
        for (index, arg) in args.iter().enumerate() {
            let kind = signature
                .required
                .get(index)
                .or_else(|| {
                    index
                        .checked_sub(signature.required.len())
                        .and_then(|optional| signature.optional.get(optional))
                })
                .copied()
                .unwrap_or(P::Any);
            nodes.push(self.param(arg, kind)?);
        }
        let first_ty = nodes.first().map(|node| node.ty);
        let ty = match signature.result {
            Res::Fixed(ty) => ty,
            Res::Arg0 => first_ty.unwrap_or(Ty::OPEN_LIST),
            Res::Item0 => first_ty.map_or(Ty::TEXT, Ty::item),
        };
        Ok(Node {
            kind: Kind::Call(f, nodes),
            ty,
        })
    }

    /// `min`, `max` (numbers or dates), `sum`, `mean`, `median` (numbers): scalars and lists mixed.
    fn numbers(&mut self, f: F, args: &[Expr]) -> Checked<Node> {
        let dates_allowed = matches!(f, F::Min | F::Max);
        let mut mode: Option<Base> = None;
        let mut nodes = Vec::with_capacity(args.len());
        for arg in args {
            let node = self.expr(arg)?;
            if let Some(base) = node.ty.base {
                let allowed = base == Base::Number || (dates_allowed && base == Base::Date);
                if !allowed {
                    return Err(type_mismatch(
                        arg,
                        Ty::of(mode.unwrap_or(Base::Number)),
                        node.ty,
                    ));
                }
                match mode {
                    None => mode = Some(base),
                    Some(seen) if seen != base => {
                        return Err(type_mismatch(arg, Ty::of(seen), node.ty));
                    }
                    Some(_) => {}
                }
            }
            nodes.push(node);
        }
        Ok(Node {
            kind: Kind::Call(f, nodes),
            ty: Ty::of(mode.unwrap_or(Base::Number)),
        })
    }

    fn concat(&mut self, args: &[Expr]) -> Checked<Node> {
        let mut acc = Ty::OPEN_LIST;
        let mut nodes = Vec::with_capacity(args.len());
        for arg in args {
            let node = self.any_list(arg)?;
            acc = acc
                .unify(node.ty)
                .ok_or_else(|| type_mismatch(arg, acc, node.ty))?;
            nodes.push(node);
        }
        Ok(Node {
            kind: Kind::Call(F::Concat, nodes),
            ty: acc,
        })
    }

    fn includes(&mut self, args: &[Expr]) -> Checked<Node> {
        let (Some(list_expr), Some(value_expr)) = (args.first(), args.get(1)) else {
            return Err(error(
                FormulaErrorCode::Arity,
                0,
                0,
                "includes takes 2 arguments".to_owned(),
                "includes:2:2".to_owned(),
            ));
        };
        let list = self.any_list(list_expr)?;
        let value = self.expr(value_expr)?;
        let item = list.ty.item();
        if value.ty.list || item.unify(value.ty).is_none() {
            return Err(type_mismatch(value_expr, item, value.ty));
        }
        Ok(Node {
            kind: Kind::Call(F::Includes, vec![list, value]),
            ty: Ty::BOOLEAN,
        })
    }

    fn equality(&mut self, f: F, args: &[Expr]) -> Checked<Node> {
        let (Some(left_expr), Some(right_expr)) = (args.first(), args.get(1)) else {
            return Err(error(
                FormulaErrorCode::Arity,
                0,
                0,
                "equal takes 2 arguments".to_owned(),
                "equal:2:2".to_owned(),
            ));
        };
        let left = self.expr(left_expr)?;
        let right = self.expr(right_expr)?;
        if left.ty.unify(right.ty).is_none() {
            return Err(type_mismatch(right_expr, left.ty, right.ty));
        }
        let op = if f == F::Equal { CmpOp::Eq } else { CmpOp::Ne };
        Ok(Node {
            kind: Kind::Cmp(op, Box::new(left), Box::new(right)),
            ty: Ty::BOOLEAN,
        })
    }

    /// `if` and `ifs`: the values (and the else) must be of one type.
    fn conditional(&mut self, args: &[Expr]) -> Checked<Node> {
        let pairs = args.len().saturating_sub(1) / 2;
        let mut arms = Vec::with_capacity(pairs);
        let mut ty: Option<Ty> = None;
        for pair in 0..pairs {
            let (Some(cond_expr), Some(value_expr)) = (args.get(2 * pair), args.get(2 * pair + 1))
            else {
                break;
            };
            let cond = self.scalar(cond_expr, Base::Boolean)?;
            let value = self.expr(value_expr)?;
            ty = Some(match ty {
                None => value.ty,
                Some(seen) => seen
                    .unify(value.ty)
                    .ok_or_else(|| type_mismatch(value_expr, seen, value.ty))?,
            });
            arms.push((cond, value));
        }
        let Some(other_expr) = args.last() else {
            return Err(error(
                FormulaErrorCode::Arity,
                0,
                0,
                "if takes 3 arguments".to_owned(),
                "if:3:3".to_owned(),
            ));
        };
        let other = self.expr(other_expr)?;
        let ty = match ty {
            None => other.ty,
            Some(seen) => seen
                .unify(other.ty)
                .ok_or_else(|| type_mismatch(other_expr, seen, other.ty))?,
        };
        Ok(Node {
            kind: Kind::Cond(arms, Box::new(other)),
            ty,
        })
    }

    /// `let` and `lets`.
    fn bind(&mut self, args: &[Expr]) -> Checked<Node> {
        let base = self.scope.len();
        let count = args.len().saturating_sub(1) / 2;
        let mut values = Vec::with_capacity(count);
        for pair in 0..count {
            let (Some(name_expr), Some(value_expr)) = (args.get(2 * pair), args.get(2 * pair + 1))
            else {
                break;
            };
            let value = self.expr(value_expr)?;
            let ExprKind::Var(name) = &name_expr.kind else {
                return Err(error(
                    FormulaErrorCode::UnknownVariable,
                    name_expr.start,
                    name_expr.end,
                    "a let name is an identifier".to_owned(),
                    String::new(),
                ));
            };
            self.scope.push((name.clone(), value.ty));
            values.push(value);
        }
        let Some(body_expr) = args.last() else {
            return Err(error(
                FormulaErrorCode::Arity,
                0,
                0,
                "let takes 3 arguments".to_owned(),
                "let:3:3".to_owned(),
            ));
        };
        let body = self.expr(body_expr);
        self.scope.truncate(base);
        let body = body?;
        let ty = body.ty;
        Ok(Node {
            kind: Kind::Lets(values, Box::new(body)),
            ty,
        })
    }

    /// `map`, `filter`, `find`, `findIndex`, `some`, `every`, `count`.
    fn list_function(&mut self, f: F, args: &[Expr]) -> Checked<Node> {
        let Some(list_expr) = args.first() else {
            return Err(error(
                FormulaErrorCode::Arity,
                0,
                0,
                "a list function takes a list".to_owned(),
                String::new(),
            ));
        };
        let list = self.any_list(list_expr)?;
        let body_expr = args.get(1);
        let base = self.scope.len();
        self.scope.push(("current".to_owned(), list.ty.item()));
        self.scope.push(("index".to_owned(), Ty::NUMBER));
        let body = body_expr.map(|expr| self.expr(expr));
        self.scope.truncate(base);
        let body = body.transpose()?;
        let boolean_body = |expr: Option<&Expr>, node: Option<&Node>| -> Checked<()> {
            match (expr, node) {
                (Some(expr), Some(node)) if !node.ty.is(Base::Boolean) => {
                    Err(type_mismatch(expr, Ty::BOOLEAN, node.ty))
                }
                _ => Ok(()),
            }
        };
        let ty = match f {
            F::Map => match (body_expr, &body) {
                (Some(expr), Some(node)) => {
                    if node.ty.list {
                        return Err(type_mismatch(expr, node.ty.item(), node.ty));
                    }
                    node.ty.many()
                }
                _ => Ty::OPEN_LIST,
            },
            F::Filter => {
                boolean_body(body_expr, body.as_ref())?;
                list.ty
            }
            F::Find => {
                boolean_body(body_expr, body.as_ref())?;
                list.ty.item()
            }
            F::FindIndex | F::Count => {
                boolean_body(body_expr, body.as_ref())?;
                Ty::NUMBER
            }
            _ => {
                boolean_body(body_expr, body.as_ref())?;
                Ty::BOOLEAN
            }
        };
        Ok(Node {
            kind: Kind::ListFn(f, Box::new(list), body.map(Box::new)),
            ty,
        })
    }

    // ---- properties

    fn property_call(&mut self, args: &[Expr]) -> Checked<Node> {
        let (receiver, literal) = match args {
            [literal] => (None, literal),
            [page, literal] => (Some(self.scalar(page, Base::Page)?), literal),
            _ => {
                return Err(error(
                    FormulaErrorCode::Arity,
                    0,
                    0,
                    "prop takes 1 or 2 arguments".to_owned(),
                    "prop:1:2".to_owned(),
                ));
            }
        };
        let ExprKind::Str(id) = &literal.kind else {
            return Err(error(
                FormulaErrorCode::UnknownProperty,
                literal.start,
                literal.end,
                "the argument of prop must be a string literal".to_owned(),
                String::new(),
            ));
        };
        let read = self.read(id, literal)?;
        let ty = read.ty;
        let kind = match receiver {
            Some(page) => Kind::PageProp(Box::new(page), Box::new(read)),
            None => Kind::Prop(Box::new(read)),
        };
        Ok(Node { kind, ty })
    }

    fn lookup(&mut self, id: &str) -> Option<FormulaProperty> {
        if let Some(known) = self.props.get(id) {
            return known.clone();
        }
        let found = self.schema.property(id);
        self.props.insert(id.to_owned(), found.clone());
        found
    }

    /// Resolves a property id against the schema and gives its type.
    fn read(&mut self, id: &str, at: &Expr) -> Checked<PropRead> {
        if id == FORMULA_TITLE_ID {
            return Ok(PropRead {
                id: id.to_owned(),
                pid: None,
                kind: ReadKind::Title,
                options: Vec::new(),
                ty: Ty::TEXT,
            });
        }
        let Some(property) = self.lookup(id) else {
            return Err(error(
                FormulaErrorCode::UnknownProperty,
                at.start,
                at.end,
                format!("no property \"{id}\""),
                id.to_owned(),
            ));
        };
        let unsupported = || {
            error(
                FormulaErrorCode::UnsupportedProperty,
                at.start,
                at.end,
                format!("\"{}\" cannot be used in a formula", property.name),
                property.name.clone(),
            )
        };
        let ty = match property.kind {
            PropertyKind::Text
            | PropertyKind::Url
            | PropertyKind::Email
            | PropertyKind::Phone
            | PropertyKind::Select
            | PropertyKind::Status
            | PropertyKind::Place => Ty::TEXT,
            PropertyKind::Number => Ty::NUMBER,
            PropertyKind::MultiSelect | PropertyKind::Files => Ty::list_of(Base::Text),
            PropertyKind::Date | PropertyKind::CreatedTime | PropertyKind::LastEditedTime => {
                Ty::DATE
            }
            PropertyKind::Checkbox => Ty::BOOLEAN,
            PropertyKind::Checklist => Ty::list_of(Base::Boolean),
            PropertyKind::Person => Ty::list_of(Base::Person),
            PropertyKind::Relation => Ty::list_of(Base::Page),
            PropertyKind::Rollup => rollup_type(&property).ok_or_else(unsupported)?,
            PropertyKind::Formula => self.formula_type(&property, at)?,
            PropertyKind::CreatedBy | PropertyKind::LastEditedBy => return Err(unsupported()),
        };
        let keeps_options = property.kind.has_options() || property.kind == PropertyKind::Rollup;
        Ok(PropRead {
            id: id.to_owned(),
            pid: id.parse::<PropertyId>().ok(),
            kind: ReadKind::Kind(property.kind),
            options: if keeps_options {
                property.options.clone()
            } else {
                Vec::new()
            },
            ty,
        })
    }

    /// The type of another formula of the schema: [`Checker::prepare`] has checked it already.
    fn formula_type(&mut self, property: &FormulaProperty, at: &Expr) -> Checked<Ty> {
        if self.self_id == Some(property.id.as_str()) || self.visiting.contains(&property.id) {
            return Err(cycle_error(at.start, at.end, &property.name));
        }
        match self.done.get(&property.id) {
            Some(Ok(node)) => Ok(node.ty),
            Some(Err(inner)) => Err(nested_error(at, property, inner)),
            None => {
                let inner = error(
                    FormulaErrorCode::Syntax,
                    0,
                    0,
                    "the formula was not checked".to_owned(),
                    String::new(),
                );
                Err(nested_error(at, property, &inner))
            }
        }
    }

    /// The ids of the formula properties an expression reads through `prop` and `.prop`, in order,
    /// without repeats.
    fn formula_refs(&mut self, root: &Expr) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        let mut pending: Vec<&Expr> = vec![root];
        while let Some(expr) = pending.pop() {
            match &expr.kind {
                ExprKind::List(items) | ExprKind::And(items) | ExprKind::Or(items) => {
                    pending.extend(items.iter().rev());
                }
                ExprKind::Neg(inner) | ExprKind::Not(inner) => pending.push(inner),
                ExprKind::Compare(_, a, b) | ExprKind::Pow(a, b) => {
                    pending.push(b);
                    pending.push(a);
                }
                ExprKind::Arith(first, rest) => {
                    pending.extend(rest.iter().rev().map(|(_, operand)| operand));
                    pending.push(first);
                }
                ExprKind::Ternary(a, b, c) => {
                    pending.push(c);
                    pending.push(b);
                    pending.push(a);
                }
                ExprKind::Call(call) => {
                    if call.name == "prop"
                        && let Some(Expr {
                            kind: ExprKind::Str(id),
                            ..
                        }) = call.args.last()
                        && !ids.contains(id)
                        && self
                            .lookup(id)
                            .is_some_and(|property| property.kind == PropertyKind::Formula)
                    {
                        ids.push(id.clone());
                    }
                    pending.extend(call.args.iter().rev());
                }
                ExprKind::Number(_) | ExprKind::Str(_) | ExprKind::Bool(_) | ExprKind::Var(_) => {}
            }
        }
        ids
    }

    /// Checks every formula property the root reaches, the ones it reads last first, so that
    /// checking an expression never has to check another one inside it (a chain of formulas is not
    /// a deep recursion). A loop is found on the way: a formula that is being checked and is read
    /// again is a `Cycle` for the expression that reads it.
    fn prepare(&mut self, root: &Expr) {
        struct Frame {
            id: String,
            ast: Expr,
            /// Still to enter, the next one last.
            pending: Vec<String>,
        }
        let mut roots = self.formula_refs(root);
        roots.reverse();
        let mut stack: Vec<Frame> = Vec::new();
        loop {
            let next = match stack.last_mut() {
                Some(frame) => frame.pending.pop(),
                None => roots.pop(),
            };
            if let Some(id) = next {
                if self.done.contains_key(&id)
                    || self.visiting.contains(&id)
                    || self.self_id == Some(id.as_str())
                {
                    continue;
                }
                let Some(property) = self.lookup(&id) else {
                    continue;
                };
                let parsed = match property.expression.as_deref() {
                    Some(text) => parser::parse(text).map_err(Box::new),
                    None => Err(error(
                        FormulaErrorCode::Syntax,
                        0,
                        0,
                        "no expression".to_owned(),
                        String::new(),
                    )),
                };
                match parsed {
                    Ok(ast) => {
                        let mut pending = self.formula_refs(&ast);
                        pending.reverse();
                        self.visiting.push(id.clone());
                        stack.push(Frame { id, ast, pending });
                    }
                    Err(inner) => {
                        self.done.insert(id, Err(inner));
                    }
                }
            } else {
                let Some(frame) = stack.pop() else {
                    break;
                };
                // The formula is checked with its own names and from the top of the tree.
                let saved_scope = std::mem::take(&mut self.scope);
                let outcome = self.expr(&frame.ast).map(Arc::new);
                self.scope = saved_scope;
                self.visiting.pop();
                self.done.insert(frame.id, outcome);
            }
        }
    }
}

/// The type of a rollup by its function (section 2.3); `None` for a rollup the schema could not
/// read.
fn rollup_type(property: &FormulaProperty) -> Option<Ty> {
    let (function, target) = property.rollup?;
    Some(match function {
        RollupFunction::Earliest | RollupFunction::Latest => Ty::DATE,
        RollupFunction::ShowOriginal => Ty::list_of(match target {
            None => Base::Text,
            Some(PropertyKind::Number) => Base::Number,
            Some(PropertyKind::Date | PropertyKind::CreatedTime | PropertyKind::LastEditedTime) => {
                Base::Date
            }
            Some(PropertyKind::Checkbox | PropertyKind::Checklist) => Base::Boolean,
            Some(PropertyKind::Person) => Base::Person,
            Some(_) => Base::Text,
        }),
        _ => Ty::NUMBER,
    })
}
