//! The static types of formulas (`formulas.md` section 2.3) and the typed tree the checker builds
//! for the evaluator.

use std::sync::Arc;

use kb_types::{FormulaBase, FormulaType, OptionId, PropertyId, PropertyKind};

use super::ast::CmpOp;
use super::functions::F;

/// A base type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Base {
    Number,
    Text,
    Boolean,
    Date,
    Page,
    Person,
}

impl Base {
    pub fn name(self) -> &'static str {
        match self {
            Base::Number => "number",
            Base::Text => "text",
            Base::Boolean => "boolean",
            Base::Date => "date",
            Base::Page => "page",
            Base::Person => "person",
        }
    }

    fn api(self) -> FormulaBase {
        match self {
            Base::Number => FormulaBase::Number,
            Base::Text => FormulaBase::Text,
            Base::Boolean => FormulaBase::Boolean,
            Base::Date => FormulaBase::Date,
            Base::Page => FormulaBase::Page,
            Base::Person => FormulaBase::Person,
        }
    }
}

/// A type: a base (or an open one: what `[]` holds, which takes the type of what it meets) and
/// whether it is a list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Ty {
    pub base: Option<Base>,
    pub list: bool,
}

impl Ty {
    pub const NUMBER: Ty = Ty::of(Base::Number);
    pub const TEXT: Ty = Ty::of(Base::Text);
    pub const BOOLEAN: Ty = Ty::of(Base::Boolean);
    pub const DATE: Ty = Ty::of(Base::Date);
    /// The type of `[]`.
    pub const OPEN_LIST: Ty = Ty {
        base: None,
        list: true,
    };

    pub const fn of(base: Base) -> Ty {
        Ty {
            base: Some(base),
            list: false,
        }
    }

    pub const fn list_of(base: Base) -> Ty {
        Ty {
            base: Some(base),
            list: true,
        }
    }

    /// A scalar of this base (or an open scalar).
    pub fn is(self, base: Base) -> bool {
        !self.list && self.base.is_none_or(|own| own == base)
    }

    /// A scalar of a known base.
    pub fn is_known(self, base: Base) -> bool {
        !self.list && self.base == Some(base)
    }

    /// The type of an item of this list type.
    pub const fn item(self) -> Ty {
        Ty {
            base: self.base,
            list: false,
        }
    }

    /// The list of this item type.
    pub const fn many(self) -> Ty {
        Ty {
            base: self.base,
            list: true,
        }
    }

    /// The type both sides agree on (an open base takes the other's), if they do.
    pub fn unify(self, other: Ty) -> Option<Ty> {
        if self.list != other.list {
            return None;
        }
        match (self.base, other.base) {
            (Some(a), Some(b)) if a != b => None,
            (Some(a), _) | (_, Some(a)) => Some(Ty {
                base: Some(a),
                list: self.list,
            }),
            (None, None) => Some(self),
        }
    }

    /// The name in an error's `detail` (`number`, `list<text>`; `list` for an open one).
    pub fn name(self) -> String {
        match (self.base, self.list) {
            (Some(base), false) => base.name().to_owned(),
            (Some(base), true) => format!("list<{}>", base.name()),
            (None, false) => "text".to_owned(),
            (None, true) => "list".to_owned(),
        }
    }

    /// The type of a formula's result: an open type is text.
    pub fn result_type(self) -> FormulaType {
        FormulaType {
            base: self.base.map_or(FormulaBase::Text, Base::api),
            list: self.list,
        }
    }
}

/// A step of an arithmetic chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Step {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    /// Text concatenation: `+` with a text side.
    Concat,
}

/// How a property is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReadKind {
    /// The page title (`prop("title")`).
    Title,
    /// A property of this kind.
    Kind(PropertyKind),
}

/// A property a formula reads, resolved against the schema.
#[derive(Debug, Clone)]
pub(super) struct PropRead {
    /// The id as the formula wrote it.
    pub id: String,
    /// The id as a `PropertyId` (`None` for the title).
    pub pid: Option<PropertyId>,
    pub kind: ReadKind,
    /// The options of a select, multi-select or status; of a rollup, the options of its target.
    pub options: Vec<(OptionId, String)>,
    /// The type the read has.
    pub ty: Ty,
}

/// A checked node: what to evaluate and its type.
#[derive(Debug, Clone)]
pub(super) struct Node {
    pub kind: Kind,
    pub ty: Ty,
}

#[derive(Debug, Clone)]
pub(super) enum Kind {
    Num(f64),
    Text(Arc<str>),
    Bool(bool),
    List(Vec<Node>),
    /// A let-bound variable or `current` / `index`: its place in the scope stack.
    Slot(usize),
    /// A property of this page.
    Prop(Box<PropRead>),
    /// A property of the page the first node gives.
    PageProp(Box<Node>, Box<PropRead>),
    Neg(Box<Node>),
    Not(Box<Node>),
    And(Vec<Node>),
    Or(Vec<Node>),
    Cmp(CmpOp, Box<Node>, Box<Node>),
    Chain(Box<Node>, Vec<(Step, Node)>),
    Pow(Box<Node>, Box<Node>),
    /// `if`, `ifs` and `?:`: the (condition, value) arms in order, then the else.
    Cond(Vec<(Node, Node)>, Box<Node>),
    /// `let` and `lets`: values bound in order, then the body.
    Lets(Vec<Node>, Box<Node>),
    /// An eager call.
    Call(F, Vec<Node>),
    /// `map`, `filter`, `find`, `findIndex`, `some`, `every`, `count`: the list and the optional
    /// body evaluated with `current` and `index` bound.
    ListFn(F, Box<Node>, Option<Box<Node>>),
}
