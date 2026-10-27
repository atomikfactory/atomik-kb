//! The syntax tree of a formula (`formulas.md` section 2.2). Every node knows its range in the text
//! (characters, `end` exclusive) for the errors of the checker.
//!
//! Left-associative operator chains are one flat node (`1 + 2 + 3 + …` is not a tree thousands of
//! levels deep) and a run of prefix operators is folded by parity, so the depth of a tree follows
//! the nesting of parentheses, calls and lists the parser bounds.

/// A node and its range.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Expr {
    pub kind: ExprKind,
    pub start: usize,
    pub end: usize,
}

/// A comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// An arithmetic operator of a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

/// A function call (a method call `x.f(a)` is `f(x, a)`).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Call {
    pub name: String,
    /// The range of the name.
    pub name_start: usize,
    pub name_end: usize,
    pub args: Vec<Expr>,
}

/// The kinds of node.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ExprKind {
    Number(f64),
    Str(String),
    Bool(bool),
    List(Vec<Expr>),
    /// A name: `current`, `index` or a let-bound variable.
    Var(String),
    Call(Box<Call>),
    Neg(Box<Expr>),
    Not(Box<Expr>),
    /// Two or more operands.
    And(Vec<Expr>),
    /// Two or more operands.
    Or(Vec<Expr>),
    Compare(CmpOp, Box<Expr>, Box<Expr>),
    /// `first op rest0 op rest1 …`, left to right, all at one precedence level.
    Arith(Box<Expr>, Vec<(ArithOp, Expr)>),
    Pow(Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
}
