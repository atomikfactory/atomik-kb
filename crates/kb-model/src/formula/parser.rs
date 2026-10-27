//! The parser (`formulas.md` section 2.2): recursive descent with precedence climbing for the binary
//! operators. Nesting (parentheses, calls, lists, the branches of `?:`, the exponent of `^`) is counted; deeper than `MAX_FORMULA_DEPTH` is `too_long`. The levels of
//! the grammar share a few small functions so that a deeply nested formula does not use much stack.

use kb_types::properties::{MAX_FORMULA_CHARS, MAX_FORMULA_DEPTH};

use super::ast::{ArithOp, Call, CmpOp, Expr, ExprKind};
use super::functions::is_reserved_name;
use super::lexer::{Tok, Token, describe, tokenize};
use super::{FormulaError, FormulaErrorCode};

/// Errors are boxed so that the `Result` of every function of the descent stays small.
type Parsed<T> = Result<T, Box<FormulaError>>;

/// Parses a whole expression.
pub(super) fn parse(text: &str) -> Result<Expr, FormulaError> {
    parse_boxed(text).map_err(|error| *error)
}

fn parse_boxed(text: &str) -> Parsed<Expr> {
    let length = text.chars().count();
    if length > MAX_FORMULA_CHARS {
        return Err(Box::new(FormulaError::new(
            FormulaErrorCode::TooLong,
            format!("the formula is longer than {MAX_FORMULA_CHARS} characters"),
            MAX_FORMULA_CHARS,
            length,
            String::new(),
        )));
    }
    let mut parser = Parser {
        toks: tokenize(text),
        pos: 0,
        depth: 0,
    };
    if parser.cur().tok == Tok::Eof {
        return Err(syntax(0, 0, "the formula is empty", "an expression"));
    }
    let expr = parser.ternary()?;
    if parser.cur().tok != Tok::Eof {
        return Err(parser.unexpected("the end of the formula"));
    }
    Ok(expr)
}

fn syntax(start: usize, end: usize, message: &str, detail: &str) -> Box<FormulaError> {
    Box::new(FormulaError::new(
        FormulaErrorCode::Syntax,
        message.to_owned(),
        start,
        end,
        detail.to_owned(),
    ))
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    depth: usize,
}

/// The binary operators by precedence (higher binds tighter): `or` 1, `and` 2, (`not` 3),
/// comparisons 4, `+ -` 5, `* / %` 6.
#[derive(Clone, Copy, PartialEq)]
enum Class {
    Or,
    And,
    Compare(CmpOp),
    Arith(ArithOp),
}

fn classify(tok: &Tok) -> Option<(u8, Class)> {
    Some(match tok {
        Tok::KwOr | Tok::OrOr => (1, Class::Or),
        Tok::KwAnd | Tok::AndAnd => (2, Class::And),
        Tok::EqEq => (4, Class::Compare(CmpOp::Eq)),
        Tok::NotEq => (4, Class::Compare(CmpOp::Ne)),
        Tok::Lt => (4, Class::Compare(CmpOp::Lt)),
        Tok::Le => (4, Class::Compare(CmpOp::Le)),
        Tok::Gt => (4, Class::Compare(CmpOp::Gt)),
        Tok::Ge => (4, Class::Compare(CmpOp::Ge)),
        Tok::Plus => (5, Class::Arith(ArithOp::Add)),
        Tok::Minus => (5, Class::Arith(ArithOp::Sub)),
        Tok::Star => (6, Class::Arith(ArithOp::Mul)),
        Tok::Slash => (6, Class::Arith(ArithOp::Div)),
        Tok::Percent => (6, Class::Arith(ArithOp::Rem)),
        _ => return None,
    })
}

impl Parser {
    /// The current token (the last one, `Eof`, repeats).
    fn cur(&self) -> &Token {
        let last = self.toks.len().saturating_sub(1);
        &self.toks[self.pos.min(last)]
    }

    fn peek_tok(&self, ahead: usize) -> Option<&Tok> {
        self.toks.get(self.pos + ahead).map(|token| &token.tok)
    }

    /// The error for the current token: a lexical error as it is, anything else as "unexpected".
    fn unexpected(&self, expected: &str) -> Box<FormulaError> {
        let token = self.cur();
        match &token.tok {
            Tok::Error(message, detail) => syntax(token.start, token.end, message, detail),
            Tok::Eof => syntax(
                token.start,
                token.end,
                &format!("unexpected end of the formula, expected {expected}"),
                expected,
            ),
            other => syntax(
                token.start,
                token.end,
                &format!("unexpected {}, expected {expected}", describe(other)),
                expected,
            ),
        }
    }

    fn expect(&mut self, tok: &Tok, shown: &str) -> Parsed<usize> {
        if &self.cur().tok == tok {
            let end = self.cur().end;
            self.pos += 1;
            Ok(end)
        } else {
            Err(self.unexpected(shown))
        }
    }

    fn enter(&mut self) -> Parsed<()> {
        self.depth += 1;
        if self.depth > MAX_FORMULA_DEPTH {
            let token = self.cur();
            return Err(Box::new(FormulaError::new(
                FormulaErrorCode::TooLong,
                format!("the formula is nested more than {MAX_FORMULA_DEPTH} levels deep"),
                token.start,
                token.end,
                String::new(),
            )));
        }
        Ok(())
    }

    /// An expression inside a construct: one more level of nesting.
    fn expr(&mut self) -> Parsed<Expr> {
        self.enter()?;
        let parsed = self.ternary();
        self.depth -= 1;
        parsed
    }

    fn ternary(&mut self) -> Parsed<Expr> {
        let cond = self.binary(1)?;
        if self.cur().tok != Tok::Question {
            return Ok(cond);
        }
        self.pos += 1;
        let then = self.expr()?;
        self.expect(&Tok::Colon, "\":\"")?;
        let other = self.expr()?;
        let (start, end) = (cond.start, other.end);
        Ok(Expr {
            kind: ExprKind::Ternary(Box::new(cond), Box::new(then), Box::new(other)),
            start,
            end,
        })
    }

    /// Operators of precedence `min` and above, left to right.
    fn binary(&mut self, min: u8) -> Parsed<Expr> {
        let mut lhs = if min <= 3 && matches!(self.cur().tok, Tok::KwNot | Tok::Bang) {
            self.not()?
        } else {
            self.unary()?
        };
        while let Some((level, class)) = classify(&self.cur().tok) {
            if level < min {
                break;
            }
            lhs = match class {
                Class::Or | Class::And => self.logic(lhs, level, class)?,
                Class::Compare(op) => self.comparison(lhs, op)?,
                Class::Arith(_) => self.arithmetic(lhs, level)?,
            };
        }
        Ok(lhs)
    }

    /// `a or b or c` and `a and b and c`: one node with every operand.
    fn logic(&mut self, first: Expr, level: u8, class: Class) -> Parsed<Expr> {
        let start = first.start;
        let mut operands = vec![first];
        while classify(&self.cur().tok) == Some((level, class)) {
            self.pos += 1;
            operands.push(self.binary(level + 1)?);
        }
        let end = operands.last().map_or(start, |last| last.end);
        let kind = if class == Class::Or {
            ExprKind::Or(operands)
        } else {
            ExprKind::And(operands)
        };
        Ok(Expr { kind, start, end })
    }

    /// At most one comparison: `a < b < c` is a syntax error.
    fn comparison(&mut self, lhs: Expr, op: CmpOp) -> Parsed<Expr> {
        self.pos += 1;
        let rhs = self.binary(5)?;
        if matches!(classify(&self.cur().tok), Some((4, _))) {
            let token = self.cur();
            return Err(syntax(
                token.start,
                token.end,
                "comparisons cannot be chained: join them with and / or",
                "and",
            ));
        }
        let (start, end) = (lhs.start, rhs.end);
        Ok(Expr {
            kind: ExprKind::Compare(op, Box::new(lhs), Box::new(rhs)),
            start,
            end,
        })
    }

    /// `first op rest op rest …` at one precedence level: one flat node.
    fn arithmetic(&mut self, first: Expr, level: u8) -> Parsed<Expr> {
        let mut rest = Vec::new();
        while let Some((l, Class::Arith(op))) = classify(&self.cur().tok) {
            if l != level {
                break;
            }
            self.pos += 1;
            rest.push((op, self.binary(level + 1)?));
        }
        let Some(last) = rest.last() else {
            return Ok(first);
        };
        let (start, end) = (first.start, last.1.end);
        Ok(Expr {
            kind: ExprKind::Arith(Box::new(first), rest),
            start,
            end,
        })
    }

    /// A run of `not` / `!` is folded by parity (`not not x` keeps two so the operand is read as a
    /// boolean either way). Its operand is a comparison or tighter.
    fn not(&mut self) -> Parsed<Expr> {
        let start = self.cur().start;
        let mut count = 0_usize;
        while matches!(self.cur().tok, Tok::KwNot | Tok::Bang) {
            self.pos += 1;
            count += 1;
        }
        let operand = self.binary(4)?;
        Ok(fold(operand, start, count, ExprKind::Not))
    }

    /// A run of `-` is folded by parity.
    fn unary(&mut self) -> Parsed<Expr> {
        let start = self.cur().start;
        let mut count = 0_usize;
        while self.cur().tok == Tok::Minus {
            self.pos += 1;
            count += 1;
        }
        let operand = self.power()?;
        Ok(fold(operand, start, count, ExprKind::Neg))
    }

    fn power(&mut self) -> Parsed<Expr> {
        let base = self.postfix()?;
        if self.cur().tok != Tok::Caret {
            return Ok(base);
        }
        self.pos += 1;
        self.enter()?;
        let exponent = self.unary();
        self.depth -= 1;
        let exponent = exponent?;
        let (start, end) = (base.start, exponent.end);
        Ok(Expr {
            kind: ExprKind::Pow(Box::new(base), Box::new(exponent)),
            start,
            end,
        })
    }

    /// A primary and its method calls. A chain is flat in the text, so its links are not levels of
    /// nesting (the checker's depth bound still applies).
    fn postfix(&mut self) -> Parsed<Expr> {
        let mut expr = self.primary()?;
        while self.cur().tok == Tok::Dot {
            self.pos += 1;
            let (name, name_start, name_end) = self.method_name()?;
            let (rest, end) = self.args()?;
            let start = expr.start;
            let mut args = Vec::with_capacity(rest.len() + 1);
            args.push(expr);
            args.extend(rest);
            check_special(&name, &args, start, end)?;
            expr = Expr {
                kind: ExprKind::Call(Box::new(Call {
                    name,
                    name_start,
                    name_end,
                    args,
                })),
                start,
                end,
            };
        }
        Ok(expr)
    }

    /// The name after a dot, up to the `(` (not consumed): `and`, `or` and `not` can be methods.
    fn method_name(&mut self) -> Parsed<(String, usize, usize)> {
        let token = self.cur();
        let name = match &token.tok {
            Tok::Ident(name) => name.clone(),
            Tok::KwNot => "not".to_owned(),
            Tok::KwAnd => "and".to_owned(),
            Tok::KwOr => "or".to_owned(),
            _ => return Err(self.unexpected("a function name")),
        };
        let (start, end) = (token.start, token.end);
        self.pos += 1;
        if self.cur().tok != Tok::LParen {
            return Err(self.unexpected("\"(\""));
        }
        Ok((name, start, end))
    }

    /// The arguments after the name: the current token is `(`. Returns them and the end of `)`.
    fn args(&mut self) -> Parsed<(Vec<Expr>, usize)> {
        self.pos += 1;
        let mut args = Vec::new();
        if self.cur().tok == Tok::RParen {
            let end = self.cur().end;
            self.pos += 1;
            return Ok((args, end));
        }
        loop {
            args.push(self.expr()?);
            match self.cur().tok {
                Tok::Comma => self.pos += 1,
                Tok::RParen => {
                    let end = self.cur().end;
                    self.pos += 1;
                    return Ok((args, end));
                }
                _ => return Err(self.unexpected("\",\" or \")\"")),
            }
        }
    }

    fn call(&mut self, name: String, name_start: usize, name_end: usize) -> Parsed<Expr> {
        self.pos += 1;
        let (args, end) = self.args()?;
        check_special(&name, &args, name_start, end)?;
        Ok(Expr {
            kind: ExprKind::Call(Box::new(Call {
                name,
                name_start,
                name_end,
                args,
            })),
            start: name_start,
            end,
        })
    }

    fn primary(&mut self) -> Parsed<Expr> {
        let (start, end) = (self.cur().start, self.cur().end);
        let literal = |kind: ExprKind| Expr { kind, start, end };
        let kind = match &self.cur().tok {
            Tok::Number(value) => ExprKind::Number(*value),
            Tok::Str(value) => ExprKind::Str(value.clone()),
            Tok::True => ExprKind::Bool(true),
            Tok::False => ExprKind::Bool(false),
            Tok::LBracket => return self.list(start),
            Tok::LParen => return self.parenthesised(start),
            Tok::Ident(name) => {
                let name = name.clone();
                if self.peek_tok(1) == Some(&Tok::LParen) {
                    return self.call(name, start, end);
                }
                ExprKind::Var(name)
            }
            // `and(a, b)` and `or(a, b)` as functions; the operators come after an operand.
            Tok::KwAnd | Tok::KwOr if self.peek_tok(1) == Some(&Tok::LParen) => {
                let name = if self.cur().tok == Tok::KwAnd {
                    "and"
                } else {
                    "or"
                };
                return self.call(name.to_owned(), start, end);
            }
            _ => return Err(self.unexpected("an expression")),
        };
        self.pos += 1;
        Ok(literal(kind))
    }

    /// `( expr )`: the node of the inner expression with the range of the parentheses.
    fn parenthesised(&mut self, start: usize) -> Parsed<Expr> {
        self.pos += 1;
        let inner = self.expr()?;
        let end = self.expect(&Tok::RParen, "\")\"")?;
        Ok(Expr {
            kind: inner.kind,
            start,
            end,
        })
    }

    fn list(&mut self, start: usize) -> Parsed<Expr> {
        self.pos += 1;
        let mut items = Vec::new();
        if self.cur().tok == Tok::RBracket {
            let end = self.cur().end;
            self.pos += 1;
            return Ok(Expr {
                kind: ExprKind::List(items),
                start,
                end,
            });
        }
        loop {
            items.push(self.expr()?);
            match self.cur().tok {
                Tok::Comma => self.pos += 1,
                Tok::RBracket => {
                    let end = self.cur().end;
                    self.pos += 1;
                    return Ok(Expr {
                        kind: ExprKind::List(items),
                        start,
                        end,
                    });
                }
                _ => return Err(self.unexpected("\",\" or \"]\"")),
            }
        }
    }
}

/// A run of `count` prefix operators as one or two nodes (the parity decides which): the operand is
/// read the way the operator reads it either way.
fn fold(operand: Expr, start: usize, count: usize, wrap: fn(Box<Expr>) -> ExprKind) -> Expr {
    if count == 0 {
        return operand;
    }
    let end = operand.end;
    let mut folded = Expr {
        kind: wrap(Box::new(operand)),
        start,
        end,
    };
    if count.is_multiple_of(2) {
        folded = Expr {
            kind: wrap(Box::new(folded)),
            start,
            end,
        };
    }
    folded
}

/// The shapes only the parser can refuse: the string literal of `prop` and the names of `let` and
/// `lets` (section 3.7). A wrong number of arguments is the checker's `arity`.
fn check_special(name: &str, args: &[Expr], start: usize, end: usize) -> Parsed<()> {
    match name {
        "prop" => {
            let literal_last = matches!(args.last().map(|last| &last.kind), Some(ExprKind::Str(_)));
            if (1..=2).contains(&args.len()) && literal_last {
                return Ok(());
            }
            let (from, to) = match args.last() {
                Some(last) if args.len() <= 2 => (last.start, last.end),
                _ => (start, end),
            };
            Err(syntax(
                from,
                to,
                "the argument of prop must be a string literal",
                "a string literal",
            ))
        }
        "let" if args.len() == 3 => args.first().map_or(Ok(()), check_binding_name),
        "lets" if args.len() >= 3 && args.len() % 2 == 1 => {
            for name in args.iter().take(args.len() - 1).step_by(2) {
                check_binding_name(name)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn check_binding_name(arg: &Expr) -> Parsed<()> {
    match &arg.kind {
        ExprKind::Var(name) if !is_reserved_name(name) => Ok(()),
        _ => Err(syntax(
            arg.start,
            arg.end,
            "a let name is an identifier that is not a function, \"current\", \"index\" or a keyword",
            "a name",
        )),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    fn kind(text: &str) -> ExprKind {
        parse(text).unwrap().kind
    }

    fn err(text: &str) -> FormulaError {
        parse(text).unwrap_err()
    }

    #[test]
    fn precedence_and_associativity() {
        // 1 + (2 * (3 ^ 2))
        let ExprKind::Arith(first, rest) = kind("1 + 2 * 3 ^ 2") else {
            panic!("a sum");
        };
        assert_eq!(first.kind, ExprKind::Number(1.0));
        assert_eq!(rest.len(), 1);
        let ExprKind::Arith(_, product) = &rest[0].1.kind else {
            panic!("a product");
        };
        assert!(matches!(product[0].1.kind, ExprKind::Pow(..)));
        // -2 ^ 2 is -(2 ^ 2); 2 ^ -1 reads a unary minus after the caret
        let ExprKind::Neg(inner) = kind("-2 ^ 2") else {
            panic!("a negation");
        };
        assert!(matches!(inner.kind, ExprKind::Pow(..)));
        let ExprKind::Pow(_, exponent) = kind("2 ^ -1") else {
            panic!("a power");
        };
        assert!(matches!(exponent.kind, ExprKind::Neg(_)));
        // ^ is right associative
        let ExprKind::Pow(_, right) = kind("2 ^ 3 ^ 2") else {
            panic!("a power");
        };
        assert!(matches!(right.kind, ExprKind::Pow(..)));
        // not binds looser than a comparison and tighter than and
        let ExprKind::And(operands) = kind("not 1 == 2 and true") else {
            panic!("an and");
        };
        assert!(matches!(operands[0].kind, ExprKind::Not(_)));
    }

    #[test]
    fn method_calls_put_the_receiver_first() {
        let ExprKind::Call(call) = kind("x.round(2)") else {
            panic!("a call");
        };
        assert_eq!(call.name, "round");
        assert_eq!(call.args.len(), 2);
        assert_eq!(call.args[0].kind, ExprKind::Var("x".into()));
        assert_eq!((call.name_start, call.name_end), (2, 7));
        let ExprKind::Call(outer) = kind("[1].first().abs()") else {
            panic!("a call");
        };
        assert_eq!(outer.name, "abs");
        assert!(matches!(&outer.args[0].kind, ExprKind::Call(inner) if inner.name == "first"));
    }

    #[test]
    fn a_ternary_nests_to_the_right() {
        let ExprKind::Ternary(_, _, other) = kind("a ? 1 : b ? 2 : 3") else {
            panic!("a ternary");
        };
        assert!(matches!(other.kind, ExprKind::Ternary(..)));
    }

    #[test]
    fn syntax_errors_and_their_positions() {
        assert_eq!(err("").start, 0);
        assert_eq!(err("  /* c */ ").code, FormulaErrorCode::Syntax);
        let trailing = err("1 +");
        assert_eq!((trailing.start, trailing.end), (3, 3));
        assert_eq!(err("max(1, 2,)").code, FormulaErrorCode::Syntax);
        assert_eq!(err("1 < 2 < 3").start, 6);
        assert_eq!(err("(1").code, FormulaErrorCode::Syntax);
        assert_eq!(err("1 2").start, 2);
        assert_eq!(err("\"abc").code, FormulaErrorCode::Syntax);
        assert_eq!(err("a ? b").code, FormulaErrorCode::Syntax);
        assert_eq!(err("[1, 2").code, FormulaErrorCode::Syntax);
        assert_eq!(err("x.").code, FormulaErrorCode::Syntax);
        assert_eq!(err("x.f").code, FormulaErrorCode::Syntax);
        assert_eq!(err("a = b").start, 2);
    }

    #[test]
    fn prop_needs_a_string_literal() {
        assert!(parse("prop(\"A\")").is_ok());
        assert!(parse("x.prop(\"A\")").is_ok());
        assert_eq!(err("prop(x)").start, 5);
        assert_eq!(err("prop()").code, FormulaErrorCode::Syntax);
        assert_eq!(
            err("prop(\"A\", \"B\", \"C\")").code,
            FormulaErrorCode::Syntax
        );
    }

    #[test]
    fn let_names_are_plain_identifiers() {
        assert!(parse("let(x, 1, x)").is_ok());
        assert!(parse("lets(a, 1, b, a, b)").is_ok());
        for text in [
            "let(\"x\", 1, 1)",
            "let(sum, 1, sum)",
            "let(current, 1, 1)",
            "let(index, 1, 1)",
            "let(true, 1, 1)",
            "let(and, 1, 1)",
            "lets(a, 1, e, 2, a)",
        ] {
            assert_eq!(err(text).code, FormulaErrorCode::Syntax, "{text}");
        }
        // names the language does not define are fine
        assert!(parse("let(link, 1, link)").is_ok());
    }

    #[test]
    fn and_or_not_work_as_functions_and_methods() {
        assert!(matches!(kind("and(true, false)"), ExprKind::Call(call) if call.name == "and"));
        assert!(matches!(kind("true.or(false)"), ExprKind::Call(call) if call.name == "or"));
        assert!(matches!(kind("true.not()"), ExprKind::Call(call) if call.name == "not"));
    }

    #[test]
    fn nesting_is_bounded() {
        let depth = |open: &str, close: &str, inner: &str, levels: usize| {
            format!("{}{inner}{}", open.repeat(levels), close.repeat(levels))
        };
        assert!(parse(&depth("(", ")", "1", MAX_FORMULA_DEPTH)).is_ok());
        assert_eq!(
            err(&depth("(", ")", "1", MAX_FORMULA_DEPTH + 1)).code,
            FormulaErrorCode::TooLong
        );
        assert!(parse(&depth("[", "]", "1", MAX_FORMULA_DEPTH)).is_ok());
        assert_eq!(
            err(&depth("[", "]", "1", MAX_FORMULA_DEPTH + 1)).code,
            FormulaErrorCode::TooLong
        );
        assert!(parse(&depth("abs(", ")", "1", MAX_FORMULA_DEPTH)).is_ok());
        assert_eq!(
            err(&depth("abs(", ")", "1", MAX_FORMULA_DEPTH + 1)).code,
            FormulaErrorCode::TooLong
        );
        // a long chain of operators is flat, not nested
        let sum = vec!["1"; 1500].join("+");
        assert!(parse(&sum).is_ok());
        // so is a run of prefix operators
        assert!(parse(&format!("{}1", "-".repeat(1500))).is_ok());
        assert!(parse(&format!("{}true", "not ".repeat(900))).is_ok());
    }

    #[test]
    fn too_long() {
        let text = "1".repeat(MAX_FORMULA_CHARS + 1);
        let error = err(&text);
        assert_eq!(error.code, FormulaErrorCode::TooLong);
        assert_eq!(
            (error.start, error.end),
            (MAX_FORMULA_CHARS, MAX_FORMULA_CHARS + 1)
        );
        assert!(parse(&format!("1{}", "+1".repeat((MAX_FORMULA_CHARS - 1) / 2))).is_ok());
        // characters, not bytes
        assert!(parse(&format!("\"{}\"", "é".repeat(MAX_FORMULA_CHARS - 2))).is_ok());
    }
}
