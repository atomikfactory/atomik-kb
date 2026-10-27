//! The values of a running formula (`formulas.md` section 2.4).

use std::rc::Rc;
use std::sync::Arc;

use kb_types::properties::MAX_FORMULA_LIST_ITEMS;
use kb_types::{AccountId, PageId};

/// A local date-time: milliseconds since 1970-01-01T00:00 local time and whether it has a time of
/// day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Date {
    pub ms: i64,
    pub time: bool,
}

/// A page and how far from the page being computed it was reached: pages of a relation of the
/// page are at level 1, `.prop` reads level 1 only (section 4.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PageRef {
    pub id: PageId,
    pub level: u8,
}

#[derive(Debug, Clone)]
pub(super) enum Value {
    Empty,
    Number(f64),
    Text(Arc<str>),
    Bool(bool),
    Date(Date),
    Page(PageRef),
    Person(AccountId),
    List(Rc<Vec<Value>>),
}

impl Value {
    pub fn text(text: &str) -> Value {
        Value::Text(Arc::from(text))
    }

    /// A number; a non-finite one is empty.
    pub fn number(value: f64) -> Value {
        if value.is_finite() {
            Value::Number(value)
        } else {
            Value::Empty
        }
    }

    /// A list; one over the limit is empty.
    pub fn list(items: Vec<Value>) -> Value {
        if items.len() > MAX_FORMULA_LIST_ITEMS {
            Value::Empty
        } else {
            Value::List(Rc::new(items))
        }
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(value) => Some(*value),
            _ => None,
        }
    }

    /// The text, or `""` for anything else (an empty text is `""`).
    pub fn as_str(&self) -> &str {
        match self {
            Value::Text(text) => text,
            _ => "",
        }
    }

    /// The items, or none for anything but a list (an empty list is `[]`).
    pub fn items(&self) -> &[Value] {
        match self {
            Value::List(items) => items,
            _ => &[],
        }
    }

    pub fn as_date(&self) -> Option<Date> {
        match self {
            Value::Date(date) => Some(*date),
            _ => None,
        }
    }

    /// How a boolean reads in `if`, `and`, `or`, `not`: an empty one is `false`.
    pub fn truthy(&self) -> bool {
        matches!(self, Value::Bool(true))
    }
}

/// `==` (section 3.2): two empties are equal, an empty and anything else are not; lists item by
/// item; dates by instant.
pub(super) fn equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Empty, Value::Empty) => true,
        (Value::Number(x), Value::Number(y)) => x == y,
        (Value::Text(x), Value::Text(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Date(x), Value::Date(y)) => x.ms == y.ms,
        (Value::Page(x), Value::Page(y)) => x.id == y.id,
        (Value::Person(x), Value::Person(y)) => x == y,
        (Value::List(x), Value::List(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| equal(p, q))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_follows_the_language() {
        assert!(equal(&Value::Empty, &Value::Empty));
        assert!(!equal(&Value::Empty, &Value::text("")));
        assert!(!equal(&Value::Empty, &Value::Number(0.0)));
        assert!(equal(&Value::Number(0.0), &Value::Number(-0.0)));
        let a = Value::list(vec![Value::Number(1.0), Value::Empty]);
        let b = Value::list(vec![Value::Number(1.0), Value::Empty]);
        assert!(equal(&a, &b));
        assert!(!equal(&a, &Value::list(vec![Value::Number(1.0)])));
        let day = |ms, time| Value::Date(Date { ms, time });
        assert!(equal(&day(5, false), &day(5, true)));
    }

    #[test]
    fn lists_are_capped_and_numbers_are_finite() {
        assert!(matches!(
            Value::list(vec![Value::Empty; MAX_FORMULA_LIST_ITEMS]),
            Value::List(_)
        ));
        assert!(matches!(
            Value::list(vec![Value::Empty; MAX_FORMULA_LIST_ITEMS + 1]),
            Value::Empty
        ));
        assert!(matches!(Value::number(f64::NAN), Value::Empty));
        assert!(matches!(Value::number(f64::INFINITY), Value::Empty));
    }
}
