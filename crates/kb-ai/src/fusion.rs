//! Reciprocal rank fusion of ranked lists (`docs/architecture/ai.md`, section 3.4).

use std::collections::{HashMap, HashSet};

/// The constant `k` of reciprocal rank fusion.
pub const RRF_K: f32 = 60.0;

/// Fuses ranked lists: each key scores `sum 1 / (k + rank + 1)` over the lists it is in (rank
/// from 0; a key repeated inside one list counts once, at its best rank); sorted by score
/// descending, then by first appearance (list order, then rank).
///
/// Scores are summed in `f64` and rounded to `f32` before they are compared, so keys that tie
/// mathematically also tie here whatever the order of the lists.
#[must_use]
pub fn rrf<K: Clone + Eq + core::hash::Hash>(lists: &[Vec<K>], k: f32) -> Vec<(K, f32)> {
    let k = f64::from(k);
    let mut slots: HashMap<&K, usize> = HashMap::new();
    let mut fused: Vec<(&K, f64)> = Vec::new();
    for list in lists {
        let mut seen: HashSet<&K> = HashSet::new();
        for (rank, key) in list.iter().enumerate() {
            if !seen.insert(key) {
                continue;
            }
            let share = 1.0 / (k + rank as f64 + 1.0);
            match slots.get(key) {
                Some(slot) => {
                    if let Some(entry) = fused.get_mut(*slot) {
                        entry.1 += share;
                    }
                }
                None => {
                    slots.insert(key, fused.len());
                    fused.push((key, share));
                }
            }
        }
    }
    let mut out: Vec<(K, f32)> = fused
        .into_iter()
        .map(|(key, score)| (key.clone(), score as f32))
        .collect();
    // `sort_by` is stable: equal scores keep their order of first appearance.
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn keys(fused: &[(&'static str, f32)]) -> Vec<&'static str> {
        fused.iter().map(|(key, _)| *key).collect()
    }

    #[test]
    fn a_key_in_both_lists_wins() {
        let fused = rrf(&[vec!["a", "b", "c"], vec!["c", "d", "a"]], RRF_K);
        // a: 1/61 + 1/63, c: 1/63 + 1/61 (equal, a appears first), b: 1/62, d: 1/62.
        assert_eq!(keys(&fused), vec!["a", "c", "b", "d"]);
        assert!((fused[0].1 - (1.0 / 61.0 + 1.0 / 63.0)).abs() < 1e-6);
        assert!((fused[2].1 - 1.0 / 62.0).abs() < 1e-6);
    }

    #[test]
    fn scores_follow_the_formula() {
        let fused = rrf(&[vec![1_u32, 2], vec![2_u32]], 60.0);
        assert_eq!(fused[0].0, 2);
        assert!((fused[0].1 - (1.0 / 62.0 + 1.0 / 61.0)).abs() < 1e-6);
        assert_eq!(fused[1].0, 1);
        assert!((fused[1].1 - 1.0 / 61.0).abs() < 1e-6);
    }

    #[test]
    fn ties_keep_first_appearance() {
        // Every key is first in exactly one list: all scores equal.
        let fused = rrf(&[vec!["x"], vec!["y"], vec!["z"]], RRF_K);
        assert_eq!(keys(&fused), vec!["x", "y", "z"]);
        let fused = rrf(&[vec!["y"], vec!["x"], vec!["z"]], RRF_K);
        assert_eq!(keys(&fused), vec!["y", "x", "z"]);
    }

    #[test]
    fn mathematical_ties_across_three_lists_stay_ties() {
        // a: ranks 0, 1, 2 and b: ranks 2, 0, 1 sum the same three terms in another order.
        let fused = rrf(
            &[
                vec!["a", "x", "b"],
                vec!["b", "a", "y"],
                vec!["z", "b", "a"],
            ],
            RRF_K,
        );
        let a = fused.iter().find(|(key, _)| *key == "a").unwrap().1;
        let b = fused.iter().find(|(key, _)| *key == "b").unwrap().1;
        assert_eq!(a.to_bits(), b.to_bits());
        assert_eq!(fused[0].0, "a");
        assert_eq!(fused[1].0, "b");
    }

    #[test]
    fn a_key_repeated_in_one_list_counts_once() {
        let fused = rrf(&[vec!["a", "a", "b"]], RRF_K);
        assert_eq!(keys(&fused), vec!["a", "b"]);
        assert!((fused[0].1 - 1.0 / 61.0).abs() < 1e-6);
        // `b` keeps its position as rank 2.
        assert!((fused[1].1 - 1.0 / 63.0).abs() < 1e-6);
    }

    #[test]
    fn empty_input_gives_empty_output() {
        assert!(rrf::<u8>(&[], RRF_K).is_empty());
        assert!(rrf::<u8>(&[vec![], vec![]], RRF_K).is_empty());
    }

    #[test]
    fn one_list_keeps_its_order() {
        let fused = rrf(&[vec![3_u8, 1, 2]], RRF_K);
        assert_eq!(
            fused.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            vec![3, 1, 2]
        );
    }
}
