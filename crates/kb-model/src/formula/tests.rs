//! The engine through its public functions over a small in-memory host: the details of the errors,
//! the limits that are not about one expression, and the result normalisation. The behaviour of the
//! functions is the corpus's (`tests/formula-cases/`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::Arc;

use kb_types::properties::{MAX_FORMULA_CHARS, MAX_FORMULA_DEPTH};
use kb_types::{
    AccountId, AttachmentId, FormulaBase, FormulaResult, FormulaType, PageId, PropertyId,
    PropertyKind, PropertyValue,
};

use super::check::{Compiled, MAX_NODE_DEPTH};
use super::types::{Kind, Node, PropRead, ReadKind, Ty};
use super::*;

fn pid(n: u16) -> PropertyId {
    let [hi, lo] = n.to_be_bytes();
    PropertyId::from_random_bytes([hi, lo, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16])
}

fn page(n: u8) -> PageId {
    PageId::from_random_bytes([n; 16])
}

/// `prop("<id>")` of property `n`.
fn p(n: u16) -> String {
    format!("prop(\"{}\")", pid(n))
}

#[derive(Default)]
struct Host {
    props: Vec<FormulaProperty>,
    values: HashMap<(PageId, PropertyId), PropertyValue>,
    titles: HashMap<PageId, String>,
    related: HashMap<(PageId, PropertyId), Vec<PageId>>,
}

impl Host {
    fn add(&mut self, n: u16, name: &str, kind: PropertyKind, expression: Option<String>) {
        self.props.push(FormulaProperty {
            id: pid(n).to_string(),
            name: name.to_owned(),
            kind,
            options: Vec::new(),
            expression,
            rollup: None,
        });
    }

    fn number(&mut self, n: u16, name: &str) {
        self.add(n, name, PropertyKind::Number, None);
    }

    fn formula(&mut self, n: u16, name: &str, expression: String) {
        self.add(n, name, PropertyKind::Formula, Some(expression));
    }
}

impl FormulaSchema for Host {
    fn property(&self, id: &str) -> Option<FormulaProperty> {
        self.props
            .iter()
            .find(|property| property.id == id)
            .cloned()
    }
}

impl FormulaHost for Host {
    fn title(&self, page: PageId) -> Option<String> {
        self.titles.get(&page).cloned()
    }
    fn value(&self, page: PageId, property: PropertyId) -> Option<PropertyValue> {
        self.values.get(&(page, property)).cloned()
    }
    fn related(&self, page: PageId, relation: PropertyId) -> Vec<PageId> {
        self.related
            .get(&(page, relation))
            .cloned()
            .unwrap_or_default()
    }
    fn created_ms(&self, _page: PageId) -> Option<i64> {
        None
    }
    fn edited_ms(&self, _page: PageId) -> Option<i64> {
        None
    }
    fn person(&self, _account: AccountId) -> Option<(String, String)> {
        None
    }
    fn file_name(&self, _attachment: AttachmentId) -> Option<String> {
        None
    }
}

const ENV: FormulaEnv = FormulaEnv {
    now_ms: 1_760_000_000_000,
    offset_seconds: 0,
};

fn eval_with(host: &Host, expression: &str, env: &FormulaEnv) -> Option<FormulaResult> {
    let parsed = parse(expression).unwrap();
    evaluate(&parsed, page(1), host, env)
}

fn eval(expression: &str) -> Option<FormulaResult> {
    eval_with(&Host::default(), expression, &ENV)
}

fn check_error(host: &Host, expression: &str) -> FormulaError {
    let parsed = parse(expression).unwrap();
    check(&parsed, host, None).unwrap_err()
}

fn error_of(expression: &str) -> (FormulaErrorCode, String) {
    match parse(expression) {
        Err(error) => (error.code, error.detail),
        Ok(parsed) => {
            let error = check(&parsed, &Host::default(), None).unwrap_err();
            (error.code, error.detail)
        }
    }
}

#[test]
fn the_details_of_the_errors() {
    use FormulaErrorCode::{
        Arity, Syntax, Type, UnknownFunction, UnknownProperty, UnknownVariable,
    };
    let cases: [(&str, FormulaErrorCode, &str); 22] = [
        ("round()", Arity, "round:1:2"),
        ("sum()", Arity, "sum:1:-1"),
        ("ifs(true, 1, false, 2)", Arity, "ifs:3:-1"),
        ("now(1)", Arity, "now:0:0"),
        ("[1].at()", Arity, "at:2:2"),
        ("1 + true", Type, "number:boolean"),
        ("[1, \"a\"]", Type, "number:text"),
        ("min(\"a\")", Type, "number:text"),
        ("length(1)", Type, "text:number"),
        ("if(true, 1, \"a\")", Type, "number:text"),
        ("abs([1])", Type, "number:list<number>"),
        ("concat([1], [\"a\"])", Type, "list<number>:list<text>"),
        ("[1].map([current])", Type, "number:list<number>"),
        ("1 == \"a\"", Type, "number:text"),
        ("join(\"a\", \",\")", Type, "list:text"),
        ("foo(1)", UnknownFunction, "foo"),
        ("x + 1", UnknownVariable, "x"),
        ("prop(\"zzz\")", UnknownProperty, "zzz"),
        ("(1", Syntax, "\")\""),
        ("max(1 2)", Syntax, "\",\" or \")\""),
        ("1 +", Syntax, "an expression"),
        ("[1].map(current + index + other)", UnknownVariable, "other"),
    ];
    for (expression, code, detail) in cases {
        assert_eq!(
            error_of(expression),
            (code, detail.to_owned()),
            "{expression}"
        );
    }
}

#[test]
fn an_error_spans_the_smallest_expression_at_fault() {
    let span = |expression: &str| {
        let error = parse(expression)
            .map(|parsed| check(&parsed, &Host::default(), None).unwrap_err())
            .unwrap_or_else(|error| error);
        (error.start, error.end)
    };
    assert_eq!(span("1 + round()"), (4, 11));
    assert_eq!(span("1 + true"), (4, 8));
    assert_eq!(span("true + 1"), (0, 4));
    assert_eq!(span("1 + 2 + true"), (8, 12));
    assert_eq!(span("\"a\" - 1 - 2"), (0, 3));
    assert_eq!(span("if(true, 1, \"a\")"), (12, 15));
    assert_eq!(span("foo(1)"), (0, 3));
    assert_eq!(span("[1].foo()"), (4, 7));
    assert_eq!(span("let(x, 1, y)"), (10, 11));
    assert_eq!(span("1 +"), (3, 3));
    // characters, not bytes or UTF-16 units
    assert_eq!(span("\"😀é\" + foo"), (7, 10));
}

#[test]
fn a_property_error_names_the_property_or_the_loop() {
    let mut host = Host::default();
    host.number(1, "Price");
    host.formula(2, "Loop A", p(3));
    host.formula(3, "Loop B", p(2));
    host.add(4, "Creator", PropertyKind::CreatedBy, None);
    host.formula(5, "Broken", "1 +".to_owned());
    let error = check_error(&host, &p(2));
    assert_eq!(error.code, FormulaErrorCode::Cycle);
    assert!(["Loop A", "Loop B"].contains(&error.detail.as_str()));
    let error = check_error(&host, &p(4));
    assert_eq!(error.code, FormulaErrorCode::UnsupportedProperty);
    assert_eq!(error.detail, "Creator");
    let error = check_error(&host, &p(5));
    assert_eq!(error.code, FormulaErrorCode::UnsupportedProperty);
    assert_eq!(error.detail, "Broken");
    let error = check_error(&host, &format!("prop(\"{}\")", pid(99)));
    assert_eq!(error.code, FormulaErrorCode::UnknownProperty);
    assert_eq!(error.detail, pid(99).to_string());
    // a formula being checked as itself is a loop, directly or through the others
    host.formula(6, "Self", format!("{} + 1", p(6)));
    let own = format!("{} + 1", p(6));
    let parsed = parse(&own).unwrap();
    let id = pid(6).to_string();
    let error = check(&parsed, &host, Some(&id)).unwrap_err();
    assert_eq!(error.code, FormulaErrorCode::Cycle);
    assert_eq!(error.detail, "Self");
    // and an expression that reads other things is fine as that formula
    let reads_price = parse(&format!("{} + 1", p(1))).unwrap();
    assert!(check(&reads_price, &host, Some(&id)).is_ok());
}

#[test]
fn check_does_not_need_the_pages_and_gives_the_result_type() {
    let mut host = Host::default();
    host.number(1, "Price");
    host.formula(2, "Twice", format!("{} * 2", p(1)));
    let parsed = parse(&format!("{} > 1", p(2))).unwrap();
    assert_eq!(
        check(&parsed, &host, None),
        Ok(FormulaType {
            base: FormulaBase::Boolean,
            list: false
        })
    );
    let open = parse("[]").unwrap();
    assert_eq!(
        check(&open, &host, None),
        Ok(FormulaType {
            base: FormulaBase::Text,
            list: true
        })
    );
}

#[test]
fn evaluating_an_expression_that_does_not_check_gives_nothing() {
    let host = Host::default();
    assert_eq!(eval_with(&host, "prop(\"nope\")", &ENV), None);
    assert_eq!(eval_with(&host, "1 + true", &ENV), None);
    assert_eq!(
        eval_with(&host, "1 + 1", &ENV),
        Some(FormulaResult::Number(2.0))
    );
}

#[test]
fn the_result_is_normalised() {
    assert_eq!(
        eval("[1, at([1], 5), 2]"),
        Some(FormulaResult::List(vec![
            FormulaResult::Number(1.0),
            FormulaResult::Number(2.0),
        ]))
    );
    assert_eq!(
        eval("[\"\", \"a\", \"\"]"),
        Some(FormulaResult::List(vec![FormulaResult::Text("a".into())]))
    );
    assert_eq!(eval("[\"\"]"), None);
    assert_eq!(eval("\"\""), None);
    assert_eq!(eval("false"), Some(FormulaResult::Boolean(false)));
    assert_eq!(
        eval("[false]"),
        Some(FormulaResult::List(vec![FormulaResult::Boolean(false)]))
    );
    // zero is a value, and a negative zero is a plain zero
    let Some(FormulaResult::Number(zero)) = eval("0 * -1") else {
        panic!("a number");
    };
    assert!(zero == 0.0 && zero.is_sign_positive());
    let Some(FormulaResult::Number(zero)) = eval("round(-0.4)") else {
        panic!("a number");
    };
    assert!(zero.is_sign_positive());
    // a date comes out as a DateValue, with a time only when it has one
    assert_eq!(
        eval("parseDate(\"2026-01-02T03:04:59\")"),
        Some(FormulaResult::Date(kb_types::DateValue {
            date: "2026-01-02".into(),
            time: Some("03:04".into())
        }))
    );
}

#[test]
fn now_and_today_follow_the_environment() {
    let host = Host::default();
    let date = |expression: &str, now_ms: i64, offset_seconds: i32| match eval_with(
        &host,
        expression,
        &FormulaEnv {
            now_ms,
            offset_seconds,
        },
    ) {
        Some(FormulaResult::Date(value)) => (value.date, value.time),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        date("now()", -1, 0),
        ("1969-12-31".into(), Some("23:59".into()))
    );
    assert_eq!(date("today()", -1, 0), ("1969-12-31".into(), None));
    assert_eq!(date("today()", 0, 0), ("1970-01-01".into(), None));
    // the offset moves the local day
    assert_eq!(date("today()", 0, -3600), ("1969-12-31".into(), None));
    assert_eq!(date("today()", 0, 25 * 3600), ("1970-01-02".into(), None));
    assert_eq!(
        date("now()", 86_399_999, 1),
        ("1970-01-02".into(), Some("00:00".into()))
    );
}

#[test]
fn an_unset_property_reads_what_its_kind_says() {
    let mut host = Host::default();
    host.number(1, "Price");
    host.add(2, "Note", PropertyKind::Text, None);
    host.add(3, "Done", PropertyKind::Checkbox, None);
    host.add(4, "Files", PropertyKind::Files, None);
    host.add(5, "When", PropertyKind::Date, None);
    let read = |expression: String| eval_with(&host, &expression, &ENV);
    assert_eq!(
        read(format!("{} == \"\"", p(2))),
        Some(FormulaResult::Boolean(true))
    );
    assert_eq!(read(p(3)), Some(FormulaResult::Boolean(false)));
    assert_eq!(
        read(format!("{}.length()", p(4))),
        Some(FormulaResult::Number(0.0))
    );
    assert_eq!(
        read(format!("empty({})", p(5))),
        Some(FormulaResult::Boolean(true))
    );
    assert_eq!(
        read(format!("{} == {}", p(1), p(1))),
        Some(FormulaResult::Boolean(true))
    );
}

#[test]
fn a_value_of_another_kind_than_the_schema_reads_as_unset() {
    let mut host = Host::default();
    host.number(1, "Price");
    host.values.insert(
        (page(1), pid(1)),
        PropertyValue::Text("not a number".into()),
    );
    assert_eq!(eval_with(&host, &p(1), &ENV), None);
    host.values
        .insert((page(1), pid(1)), PropertyValue::Number(4.0));
    assert_eq!(
        eval_with(&host, &p(1), &ENV),
        Some(FormulaResult::Number(4.0))
    );
    // a number that is not finite is not a value
    host.values
        .insert((page(1), pid(1)), PropertyValue::Number(f64::INFINITY));
    assert_eq!(eval_with(&host, &p(1), &ENV), None);
}

#[test]
fn a_chain_of_formulas_is_followed_to_a_depth() {
    let chain = |length: u16| {
        let mut host = Host::default();
        host.number(1, "Base");
        host.values
            .insert((page(1), pid(1)), PropertyValue::Number(1.0));
        for n in 2..=length + 1 {
            host.formula(n, "Step", format!("{} + 1", p(n - 1)));
        }
        let top = format!("{} * 1", p(length + 1));
        eval_with(&host, &top, &ENV)
    };
    assert_eq!(chain(100), Some(FormulaResult::Number(101.0)));
    // beyond the depth the evaluation stops: no value, and no overflow of the stack
    assert_eq!(chain(1000), None);
}

#[test]
fn the_formulas_that_are_reached_share_one_budget() {
    let mut host = Host::default();
    // 3,000 items: about 4 steps an item
    host.formula(
        1,
        "Long",
        "repeat(\"a,\", 2999).split(\",\").map(1).sum()".to_owned(),
    );
    let uses = |count: usize| {
        let sum = (0..count).map(|_| p(1)).collect::<Vec<_>>().join(" + ");
        eval_with(&host, &sum, &ENV)
    };
    assert_eq!(uses(1), Some(FormulaResult::Number(3000.0)));
    assert_eq!(uses(5), Some(FormulaResult::Number(15000.0)));
    // each use costs its steps again; together they are over 100,000 and the whole formula is empty
    assert_eq!(uses(20), None);
}

#[test]
fn the_stack_of_formulas_being_evaluated_breaks_a_loop() {
    // `check` refuses a loop, so the evaluator's own guard is tested on a tree built by hand: a
    // formula that reads itself.
    let id = pid(1).to_string();
    let read = PropRead {
        id: id.clone(),
        pid: Some(pid(1)),
        kind: ReadKind::Kind(PropertyKind::Formula),
        options: Vec::new(),
        ty: Ty::NUMBER,
    };
    let reads_itself = Node {
        kind: Kind::Chain(
            Box::new(Node {
                kind: Kind::Prop(Box::new(read)),
                ty: Ty::NUMBER,
            }),
            vec![(
                super::types::Step::Add,
                Node {
                    kind: Kind::Num(1.0),
                    ty: Ty::NUMBER,
                },
            )],
        ),
        ty: Ty::NUMBER,
    };
    let mut nested = HashMap::new();
    nested.insert(id, Arc::new(reads_itself.clone()));
    let compiled = Compiled {
        root: reads_itself,
        nested,
    };
    // the inner read of the formula that is already being evaluated is empty, so the whole sum is
    assert_eq!(
        super::eval::run(&compiled, page(1), &Host::default(), &ENV),
        None
    );
}

#[test]
fn the_deepest_tree_is_bounded() {
    const { assert!(MAX_NODE_DEPTH >= 192, "64 levels of three nodes must fit") };
    let deep = format!("{}1{}", "abs(1 + ".repeat(60), ")".repeat(60));
    assert_eq!(eval(&deep), Some(FormulaResult::Number(61.0)));
}

#[test]
fn errors_display_their_code_and_range() {
    let error = parse("1 +").unwrap_err();
    assert_eq!(
        error.to_string(),
        "syntax at 3..3: unexpected end of the formula, expected an expression"
    );
    assert_eq!(
        FormulaErrorCode::UnsupportedProperty.as_str(),
        "unsupported_property"
    );
    assert_eq!(FormulaErrorCode::TooLong.as_str(), "too_long");
}

#[test]
fn a_parsed_formula_can_be_cloned_and_compared() {
    let a = parse("1 + 2").unwrap();
    assert_eq!(a.clone(), a);
    assert_ne!(a, parse("1 + 3").unwrap());
    // the same text in two forms of spacing is the same tree apart from the ranges
    assert!(format!("{a:?}").contains("Arith"));
}

/// `open` repeated `levels` times, `inner`, then `close` repeated `levels` times.
fn nested(open: &str, inner: &str, close: &str, levels: usize) -> String {
    format!("{}{inner}{}", open.repeat(levels), close.repeat(levels))
}

#[test]
fn the_deepest_and_longest_accepted_inputs_fit_a_1_mib_stack() {
    // A debug build of every function of the descent, on a thread with 1 MiB (a Windows test
    // thread has 2 MiB, the main thread 1 MiB): nothing the limits allow may overflow the stack.
    std::thread::Builder::new()
        .stack_size(1 << 20)
        .spawn(|| {
            let number = |value: f64| Some(FormulaResult::Number(value));
            let levels = MAX_FORMULA_DEPTH;
            // Nesting, 64 levels of each construct: parentheses, calls, `if`, `let`, `?:`, `^`,
            // list functions (parse, check and evaluate all recurse).
            assert_eq!(eval(&nested("(1 + ", "1", ")", levels)), number(65.0));
            assert_eq!(
                eval(&nested("abs(1 + ", "1", ")", levels - 1)),
                number(64.0)
            );
            assert_eq!(eval(&nested("if(true, ", "1", ", 0)", levels)), number(1.0));
            assert_eq!(eval(&nested("let(x, 1, ", "x", ")", levels)), number(1.0));
            assert_eq!(eval(&nested("true ? ", "1", " : 0", levels)), number(1.0));
            assert_eq!(eval(&format!("1{}", "^1".repeat(levels))), number(1.0));
            assert_eq!(
                eval(&nested("[1].map(1 + ", "1", ").sum()", levels - 1)),
                number(64.0)
            );
            assert_eq!(
                eval(&nested("max(1, [1].map(", "1", ").sum())", levels / 2)),
                number(1.0)
            );
            // One more level is refused by the parser; a list of lists by the checker.
            assert_eq!(
                parse(&nested("(", "1", ")", levels + 1)).unwrap_err().code,
                FormulaErrorCode::TooLong
            );
            assert_eq!(
                parse(&nested("[", "1", "]", levels + 1)).unwrap_err().code,
                FormulaErrorCode::TooLong
            );
            let lists = parse(&nested("[", "1", "]", levels)).unwrap();
            assert_eq!(
                check(&lists, &Host::default(), None).unwrap_err().code,
                FormulaErrorCode::Type
            );
            // Chains that are flat in the text: no recursion in the parser or the evaluator.
            assert_eq!(eval(&vec!["1"; 1900].join("+")), number(1900.0));
            assert_eq!(eval(&vec!["2"; 1900].join("*")), None);
            assert_eq!(eval(&format!("{}3", "-".repeat(1900))), number(3.0));
            assert_eq!(
                eval(&format!("{}true", "not ".repeat(900))),
                Some(FormulaResult::Boolean(true))
            );
            assert_eq!(
                eval(&format!("{}true", "!".repeat(3995))),
                Some(FormulaResult::Boolean(false))
            );
            assert_eq!(
                eval(&vec!["true"; 400].join(" and ")),
                Some(FormulaResult::Boolean(true))
            );
            assert_eq!(
                eval(&vec!["false"; 400].join(" || ")),
                Some(FormulaResult::Boolean(false))
            );
            assert_eq!(
                eval(&format!("length([{}])", vec!["1"; 1900].join(","))),
                number(1900.0)
            );
            assert_eq!(
                eval(&format!("max({})", vec!["1"; 1900].join(","))),
                number(1.0)
            );
            assert_eq!(
                eval(&format!("length({})", vec!["\"a\""; 600].join(" + "))),
                number(600.0)
            );
            // A chain of method calls is followed as far as the depth bound and no further.
            assert_eq!(eval(&format!("1{}", ".abs()".repeat(300))), number(1.0));
            assert_eq!(eval(&format!("1{}", ".round(1)".repeat(300))), number(1.0));
            assert_eq!(eval(&format!("1{}", ".abs()".repeat(600))), None);
            let long = format!("1{}", ".abs()".repeat(600));
            let parsed = parse(&long).unwrap();
            assert_eq!(
                check(&parsed, &Host::default(), None).unwrap_err().code,
                FormulaErrorCode::TooLong
            );
            // The text at its longest, through the conversions as well.
            let longest = format!("1{}", "+1".repeat(1999));
            assert_eq!(longest.len(), 3999);
            assert_eq!(eval(&longest), number(2000.0));
            let props = (0..400)
                .map(|_| "prop(\"a\")")
                .collect::<Vec<_>>()
                .join("+");
            assert!(props.chars().count() <= MAX_FORMULA_CHARS);
            let map = |name: &str| (name == "a").then(|| "b".to_owned());
            assert_eq!(
                to_stored(&props, &|name| map(name))
                    .unwrap()
                    .matches("prop(\"b\")")
                    .count(),
                400
            );
            assert_eq!(
                to_display(&props, &|name| map(name))
                    .matches("prop(\"b\")")
                    .count(),
                400
            );
            assert_eq!(
                remap_ids(&props, &|name| map(name))
                    .matches("prop(\"b\")")
                    .count(),
                400
            );
            assert_eq!(referenced_ids(&props), ["a"]);
            // Formulas that read one another: the chain of the properties a space can hold, then far
            // more than that (no value, no overflow).
            let formulas = |length: u16| {
                let mut host = Host::default();
                host.number(1, "Base");
                host.values
                    .insert((page(1), pid(1)), PropertyValue::Number(1.0));
                for n in 2..=length + 1 {
                    host.formula(n, "Step", format!("{} + 1", p(n - 1)));
                }
                eval_with(&host, &format!("{} * 1", p(length + 1)), &ENV)
            };
            assert_eq!(formulas(100), number(101.0));
            assert_eq!(formulas(1000), None);
            // The same formulas, each one nested deeply (the depth bound is shared by all of them).
            let mut host = Host::default();
            host.number(1, "Base");
            host.values
                .insert((page(1), pid(1)), PropertyValue::Number(1.0));
            for n in 2..=9 {
                host.formula(n, "Step", nested("abs(1 + ", &p(n - 1), ")", 10));
            }
            assert_eq!(eval_with(&host, &p(9), &ENV), number(81.0));
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn a_formula_can_be_shared_between_threads_and_cloned_with_its_cache() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Formula>();
    let mut host = Host::default();
    host.number(1, "Price");
    host.values
        .insert((page(1), pid(1)), PropertyValue::Number(4.0));
    let formula = parse(&format!("{} * 2", p(1))).unwrap();
    assert_eq!(
        evaluate(&formula, page(1), &host, &ENV),
        Some(FormulaResult::Number(8.0))
    );
    // a clone made after the first evaluation keeps the checked tree: it reads the values and
    // needs no schema (a host that knows no property proves it)
    let bare = Host {
        values: host.values.clone(),
        ..Host::default()
    };
    let clone = formula.clone();
    assert_eq!(clone, formula);
    assert_eq!(
        evaluate(&clone, page(1), &bare, &ENV),
        Some(FormulaResult::Number(8.0))
    );
    // from another thread
    let from_thread = std::thread::scope(|scope| {
        scope
            .spawn(|| evaluate(&formula, page(1), &bare, &ENV))
            .join()
            .unwrap()
    });
    assert_eq!(from_thread, Some(FormulaResult::Number(8.0)));
    // a formula that does not check keeps saying so
    let bad = parse("prop(\"nope\")").unwrap();
    assert_eq!(evaluate(&bad, page(1), &host, &ENV), None);
    assert_eq!(evaluate(&bad, page(1), &host, &ENV), None);
}

/// Formulas are evaluated for every page on every read of the tree: three formulas (one reads
/// another, one maps over a relation) on 2,000 pages.
#[test]
fn three_formulas_on_two_thousand_pages_are_fast() {
    const PAGES: usize = 2000;
    let id_of = |n: usize| {
        let [high, low] = u16::try_from(n).unwrap().to_be_bytes();
        PageId::from_random_bytes([high, low, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14])
    };
    let mut host = Host::default();
    host.number(1, "Price");
    host.number(2, "Hours");
    host.add(3, "Tasks", PropertyKind::Relation, None);
    host.formula(4, "Total", format!("{} * 2", p(1)));
    let pages: Vec<PageId> = (0..PAGES).map(&id_of).collect();
    for (n, page) in pages.iter().enumerate() {
        host.titles.insert(*page, format!("Page {n}"));
        host.values
            .insert((*page, pid(1)), PropertyValue::Number(n as f64));
        host.values
            .insert((*page, pid(2)), PropertyValue::Number(1.5));
        let tasks = (1..=3).map(|step| pages[(n + step) % PAGES]).collect();
        host.related.insert((*page, pid(3)), tasks);
    }
    let nested = parse(&format!("{} + 1", p(4))).unwrap();
    let mapped = parse(&format!("{}.map(current.prop(\"{}\")).sum()", p(3), pid(2))).unwrap();
    let plain = parse(&format!(
        "if({} > 10, \"big\", \"small\") + format({})",
        p(1),
        p(1)
    ))
    .unwrap();
    let started = std::time::Instant::now();
    let mut checksum = 0.0;
    for (n, page) in pages.iter().enumerate() {
        let Some(FormulaResult::Number(total)) = evaluate(&nested, *page, &host, &ENV) else {
            // page 0 has a price of zero: a number all the same
            panic!("a number for page {n}");
        };
        checksum += total;
        assert_eq!(
            evaluate(&mapped, *page, &host, &ENV),
            Some(FormulaResult::Number(4.5))
        );
        assert!(matches!(
            evaluate(&plain, *page, &host, &ENV),
            Some(FormulaResult::Text(_))
        ));
    }
    let elapsed = started.elapsed();
    // about 23 ms in a debug build and 4 ms in a release build on the development machine
    // the sum of 2n + 1 over the pages
    assert_eq!(checksum, (PAGES * (PAGES - 1) + PAGES) as f64);
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "too slow: {elapsed:?}"
    );
}
