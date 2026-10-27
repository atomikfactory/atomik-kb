//! The arithmetic of rollups (ADR-0027): a summary function over the target values of the
//! related pages. Pure: the caller (`StructureDoc`) finds the related pages and their values.
//! The TypeScript mirror (`core-client`, `rollup.ts`) computes the same results.

use std::collections::HashSet;

use kb_types::{DateValue, PropertyValue, RollupFunction, RollupResult};

/// Evaluates `function` over the target values of the related pages: one entry per related page
/// (live and existing), `None` for a page that has no value for the target (for the title: a
/// blank one). Counts of nothing are 0, a percent of nothing and a summary of no numbers or dates
/// have no result (`None`), as has a function applied to a value of the wrong kind (the caller has
/// checked the configuration already; a value of another kind counts as empty).
#[must_use]
pub(crate) fn evaluate(
    function: RollupFunction,
    values: &[Option<PropertyValue>],
) -> Option<RollupResult> {
    let total = values.len();
    let present: Vec<&PropertyValue> = values.iter().flatten().collect();
    let with_value = present.len();
    let number = |value: f64| Some(RollupResult::Number(value));
    let percent = |part: usize| {
        (total > 0).then(|| RollupResult::Number(to_f64(part) * 100.0 / to_f64(total)))
    };
    match function {
        RollupFunction::CountAll => number(to_f64(total)),
        RollupFunction::CountValues => number(to_f64(with_value)),
        RollupFunction::CountEmpty => number(to_f64(total - with_value)),
        RollupFunction::PercentEmpty => percent(total - with_value),
        RollupFunction::PercentNotEmpty => percent(with_value),
        RollupFunction::CountUnique => {
            let mut seen: HashSet<String> = HashSet::new();
            for value in &present {
                match value {
                    PropertyValue::MultiSelect(options) => {
                        seen.extend(options.iter().map(|option| format!("option:{option}")));
                    }
                    other => {
                        seen.insert(serde_json::to_string(other).unwrap_or_default());
                    }
                }
            }
            number(to_f64(seen.len()))
        }
        RollupFunction::Sum => number(numbers(&present).iter().sum()),
        RollupFunction::Average => {
            let all = numbers(&present);
            (!all.is_empty())
                .then(|| RollupResult::Number(all.iter().sum::<f64>() / to_f64(all.len())))
        }
        RollupFunction::Median => {
            let mut all = numbers(&present);
            all.sort_by(f64::total_cmp);
            let middle = all.len() / 2;
            match (
                all.get(middle),
                middle.checked_sub(1).and_then(|i| all.get(i)),
            ) {
                (Some(high), Some(low)) if all.len().is_multiple_of(2) => {
                    number((low + high) / 2.0)
                }
                (Some(mid), _) => number(*mid),
                _ => None,
            }
        }
        RollupFunction::Min => numbers(&present)
            .into_iter()
            .min_by(f64::total_cmp)
            .map(RollupResult::Number),
        RollupFunction::Max => numbers(&present)
            .into_iter()
            .max_by(f64::total_cmp)
            .map(RollupResult::Number),
        RollupFunction::Range => {
            let all = numbers(&present);
            let min = all.iter().copied().min_by(f64::total_cmp)?;
            let max = all.iter().copied().max_by(f64::total_cmp)?;
            number(max - min)
        }
        RollupFunction::Earliest => dates(&present)
            .into_iter()
            .min_by(|a, b| date_key(a).cmp(&date_key(b)))
            .map(|date| RollupResult::Date(date.clone())),
        RollupFunction::Latest => dates(&present)
            .into_iter()
            .max_by(|a, b| date_key(a).cmp(&date_key(b)))
            .map(|date| RollupResult::Date(date.clone())),
        RollupFunction::CountChecked => number(to_f64(checked(&present))),
        RollupFunction::PercentChecked => percent(checked(&present)),
        RollupFunction::ShowOriginal => (!present.is_empty())
            .then(|| RollupResult::Values(present.iter().map(|value| (*value).clone()).collect())),
        RollupFunction::ChecklistProgress => {
            let (mut done, mut items) = (0_usize, 0_usize);
            for value in &present {
                if let PropertyValue::Checklist(list) = value {
                    items += list.len();
                    done += list.iter().filter(|item| item.done).count();
                }
            }
            (items > 0).then(|| RollupResult::Progress {
                done: u32::try_from(done).unwrap_or(u32::MAX),
                total: u32::try_from(items).unwrap_or(u32::MAX),
            })
        }
    }
}

/// A count as an `f64` (exact below 2^53; there are at most a few thousand).
#[allow(clippy::cast_precision_loss)]
fn to_f64(count: usize) -> f64 {
    count as f64
}

fn numbers(values: &[&PropertyValue]) -> Vec<f64> {
    values
        .iter()
        .filter_map(|value| match value {
            PropertyValue::Number(number) => Some(*number),
            _ => None,
        })
        .collect()
}

fn dates<'a>(values: &[&'a PropertyValue]) -> Vec<&'a DateValue> {
    values
        .iter()
        .filter_map(|value| match value {
            PropertyValue::Date(date) => Some(date),
            _ => None,
        })
        .collect()
}

/// A date sorts by day, then time; a day without a time comes before the same day with one.
fn date_key(date: &DateValue) -> (&str, &str) {
    (date.date.as_str(), date.time.as_deref().unwrap_or(""))
}

fn checked(values: &[&PropertyValue]) -> usize {
    values
        .iter()
        .filter(|value| matches!(value, PropertyValue::Checkbox(true)))
        .count()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use kb_types::{ChecklistItem, ChecklistItemId, OptionId};

    use super::*;

    fn n(value: f64) -> Option<PropertyValue> {
        Some(PropertyValue::Number(value))
    }

    fn day(date: &str, time: Option<&str>) -> Option<PropertyValue> {
        Some(PropertyValue::Date(DateValue {
            date: date.into(),
            time: time.map(Into::into),
        }))
    }

    fn number_of(function: RollupFunction, values: &[Option<PropertyValue>]) -> Option<f64> {
        match evaluate(function, values) {
            Some(RollupResult::Number(value)) => Some(value),
            other => {
                assert!(other.is_none(), "{function:?}: {other:?}");
                None
            }
        }
    }

    #[test]
    fn counts_include_empty_values_and_zero_pages_count_zero() {
        let values = [n(1.0), None, n(1.0), n(2.0)];
        let count = |function| number_of(function, &values);
        assert_eq!(count(RollupFunction::CountAll), Some(4.0));
        assert_eq!(count(RollupFunction::CountValues), Some(3.0));
        assert_eq!(count(RollupFunction::CountEmpty), Some(1.0));
        assert_eq!(count(RollupFunction::CountUnique), Some(2.0));
        assert_eq!(count(RollupFunction::PercentEmpty), Some(25.0));
        assert_eq!(count(RollupFunction::PercentNotEmpty), Some(75.0));
        for function in [
            RollupFunction::CountAll,
            RollupFunction::CountValues,
            RollupFunction::CountEmpty,
            RollupFunction::CountUnique,
            RollupFunction::CountChecked,
            RollupFunction::Sum,
        ] {
            assert_eq!(number_of(function, &[]), Some(0.0), "{function:?}");
        }
        for function in [
            RollupFunction::PercentEmpty,
            RollupFunction::PercentNotEmpty,
            RollupFunction::PercentChecked,
            RollupFunction::Average,
            RollupFunction::Median,
            RollupFunction::Min,
            RollupFunction::Max,
            RollupFunction::Range,
            RollupFunction::Earliest,
            RollupFunction::Latest,
            RollupFunction::ShowOriginal,
            RollupFunction::ChecklistProgress,
        ] {
            assert_eq!(evaluate(function, &[]), None, "{function:?}");
        }
    }

    #[test]
    fn number_summaries_skip_pages_without_a_value() {
        let values = [n(4.0), None, n(1.0), n(10.0), n(5.0)];
        let sum = |function| number_of(function, &values);
        assert_eq!(sum(RollupFunction::Sum), Some(20.0));
        assert_eq!(sum(RollupFunction::Average), Some(5.0));
        assert_eq!(sum(RollupFunction::Median), Some(4.5));
        assert_eq!(sum(RollupFunction::Min), Some(1.0));
        assert_eq!(sum(RollupFunction::Max), Some(10.0));
        assert_eq!(sum(RollupFunction::Range), Some(9.0));
        let odd = [n(3.0), n(1.0), n(2.0)];
        assert_eq!(number_of(RollupFunction::Median, &odd), Some(2.0));
        // Pages with no number at all: the sum is 0, the others have no result.
        let blank = [None, None];
        assert_eq!(number_of(RollupFunction::Sum, &blank), Some(0.0));
        assert_eq!(evaluate(RollupFunction::Average, &blank), None);
        assert_eq!(evaluate(RollupFunction::Max, &blank), None);
    }

    #[test]
    fn dates_sort_by_day_then_time() {
        let values = [
            day("2026-10-09", None),
            day("2026-10-07", Some("14:30")),
            None,
            day("2026-10-07", None),
        ];
        assert_eq!(
            evaluate(RollupFunction::Earliest, &values),
            Some(RollupResult::Date(DateValue {
                date: "2026-10-07".into(),
                time: None
            }))
        );
        assert_eq!(
            evaluate(RollupFunction::Latest, &values),
            Some(RollupResult::Date(DateValue {
                date: "2026-10-09".into(),
                time: None
            }))
        );
    }

    #[test]
    fn checkboxes_count_unset_pages_as_unchecked() {
        let values = [
            Some(PropertyValue::Checkbox(true)),
            Some(PropertyValue::Checkbox(false)),
            None,
            Some(PropertyValue::Checkbox(true)),
        ];
        assert_eq!(number_of(RollupFunction::CountChecked, &values), Some(2.0));
        assert_eq!(
            number_of(RollupFunction::PercentChecked, &values),
            Some(50.0)
        );
        assert_eq!(number_of(RollupFunction::CountValues, &values), Some(3.0));
    }

    #[test]
    fn unique_counts_split_multi_selects_into_their_options() {
        let option = |n: u8| OptionId::from_bytes([n; 16]);
        let values = [
            Some(PropertyValue::MultiSelect(vec![option(1), option(2)])),
            Some(PropertyValue::MultiSelect(vec![option(2), option(3)])),
            None,
        ];
        assert_eq!(number_of(RollupFunction::CountUnique, &values), Some(3.0));
        let texts = [
            Some(PropertyValue::Text("a".into())),
            Some(PropertyValue::Text("b".into())),
            Some(PropertyValue::Text("a".into())),
        ];
        assert_eq!(number_of(RollupFunction::CountUnique, &texts), Some(2.0));
    }

    #[test]
    fn show_original_lists_the_values_in_order_and_checklists_sum_their_progress() {
        let values = [
            Some(PropertyValue::Text("b".into())),
            None,
            Some(PropertyValue::Text("a".into())),
        ];
        assert_eq!(
            evaluate(RollupFunction::ShowOriginal, &values),
            Some(RollupResult::Values(vec![
                PropertyValue::Text("b".into()),
                PropertyValue::Text("a".into())
            ]))
        );
        let item = |n: u8, done: bool| ChecklistItem {
            id: ChecklistItemId::from_bytes([n; 16]),
            text: String::new(),
            done,
        };
        let lists = [
            Some(PropertyValue::Checklist(vec![
                item(1, true),
                item(2, false),
            ])),
            None,
            Some(PropertyValue::Checklist(vec![item(3, true)])),
        ];
        assert_eq!(
            evaluate(RollupFunction::ChecklistProgress, &lists),
            Some(RollupResult::Progress { done: 2, total: 3 })
        );
    }
}
