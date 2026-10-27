//! Vectors of the semantic index (`docs/architecture/ai.md`, section 3.4).

/// Little-endian bytes of a vector (what `ai_chunks.vector` stores).
#[must_use]
pub fn encode(vector: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vector.len() * 4);
    for value in vector {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out
}

/// The vector of [`encode`]d bytes; `None` when the length is not a multiple of 4.
#[must_use]
pub fn decode(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    Some(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|quad| f32::from_le_bytes(*quad))
            .collect(),
    )
}

/// Scales a vector to unit length; a zero vector (or one whose length is not a finite number)
/// stays as it is.
pub fn normalize(vector: &mut [f32]) {
    let norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if norm > 0.0 && norm.is_finite() {
        for value in vector.iter_mut() {
            *value = (f64::from(*value) / norm) as f32;
        }
    }
}

/// Dot product over the common length.
#[must_use]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The `k` best candidates by dot product with `query` (already normalised), best first, ties
/// in input order; candidates of another length than the query (and candidates whose score is
/// not a number) are skipped. Returns `(index in the input, score)`.
#[must_use]
pub fn top_k<'a>(
    query: &[f32],
    candidates: impl IntoIterator<Item = &'a [f32]>,
    k: usize,
) -> Vec<(usize, f32)> {
    let mut best: Vec<(usize, f32)> = Vec::with_capacity(k.min(64));
    if k == 0 {
        return best;
    }
    for (index, candidate) in candidates.into_iter().enumerate() {
        if candidate.len() != query.len() {
            continue;
        }
        let score = dot(query, candidate);
        if score.is_nan() {
            continue;
        }
        if best.len() == k && best.last().is_some_and(|last| score <= last.1) {
            // Not better than the worst kept (a tie keeps the earlier candidate).
            continue;
        }
        // After every entry that scores at least as much: equal scores keep input order.
        let at = best.partition_point(|entry| entry.1 >= score);
        best.insert(at, (index, score));
        best.truncate(k);
    }
    best
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn encode_is_little_endian_and_round_trips() {
        let vector = [1.0_f32, -2.5, 0.0, 3.25e-3];
        let bytes = encode(&vector);
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[..4], &1.0_f32.to_le_bytes());
        assert_eq!(decode(&bytes).unwrap(), vector);
        assert_eq!(decode(&[]).unwrap(), Vec::<f32>::new());
    }

    #[test]
    fn decode_refuses_a_length_that_is_not_a_multiple_of_four() {
        assert_eq!(decode(&[1, 2, 3]), None);
        assert_eq!(decode(&[0; 5]), None);
        assert!(decode(&[0; 8]).is_some());
    }

    #[test]
    fn normalize_gives_unit_length() {
        let mut vector = [3.0_f32, 4.0];
        normalize(&mut vector);
        assert!((vector[0] - 0.6).abs() < 1e-6);
        assert!((vector[1] - 0.8).abs() < 1e-6);
        assert!((dot(&vector, &vector) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_zero_vector_stays_zero() {
        let mut vector = [0.0_f32; 4];
        normalize(&mut vector);
        assert_eq!(vector, [0.0; 4]);
        let mut empty: [f32; 0] = [];
        normalize(&mut empty);
    }

    #[test]
    fn a_huge_vector_is_normalised_without_overflow() {
        let mut vector = [1.0e30_f32, 1.0e30];
        normalize(&mut vector);
        assert!((dot(&vector, &vector) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn dot_uses_the_common_length() {
        assert_eq!(dot(&[1.0, 2.0, 3.0], &[4.0, 5.0]), 14.0);
        assert_eq!(dot(&[], &[1.0]), 0.0);
    }

    #[test]
    fn top_k_orders_by_score_and_returns_indices() {
        let candidates: Vec<Vec<f32>> = vec![
            vec![0.0, 1.0],
            vec![1.0, 0.0],
            vec![0.6, 0.8],
            vec![-1.0, 0.0],
        ];
        let query = [1.0, 0.0];
        let top = top_k(&query, candidates.iter().map(Vec::as_slice), 3);
        assert_eq!(
            top.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            vec![1, 2, 0]
        );
        assert!((top[0].1 - 1.0).abs() < 1e-6);
        assert!((top[1].1 - 0.6).abs() < 1e-6);
    }

    #[test]
    fn top_k_is_stable_on_ties() {
        let candidates: Vec<Vec<f32>> = vec![
            vec![0.5],
            vec![1.0],
            vec![0.5],
            vec![1.0],
            vec![0.5],
            vec![1.0],
        ];
        let top = top_k(&[1.0], candidates.iter().map(Vec::as_slice), 4);
        assert_eq!(
            top.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            vec![1, 3, 5, 0]
        );
        // The same with k larger than the candidates.
        let all = top_k(&[1.0], candidates.iter().map(Vec::as_slice), 99);
        assert_eq!(
            all.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            vec![1, 3, 5, 0, 2, 4]
        );
    }

    #[test]
    fn top_k_skips_candidates_of_another_length() {
        let candidates: Vec<Vec<f32>> = vec![vec![1.0], vec![1.0, 0.0], vec![0.5, 0.5, 0.5]];
        let top = top_k(&[1.0, 0.0], candidates.iter().map(Vec::as_slice), 5);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].0, 1);
    }

    #[test]
    fn top_k_handles_zero_k_empty_input_and_nan() {
        let candidates: Vec<Vec<f32>> = vec![vec![1.0]];
        assert!(top_k(&[1.0], candidates.iter().map(Vec::as_slice), 0).is_empty());
        assert!(top_k(&[1.0], std::iter::empty::<&[f32]>(), 3).is_empty());
        let with_nan: Vec<Vec<f32>> = vec![vec![f32::NAN], vec![0.5]];
        let top = top_k(&[1.0], with_nan.iter().map(Vec::as_slice), 3);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].0, 1);
    }

    #[test]
    fn top_k_keeps_the_best_when_the_list_is_full() {
        let candidates: Vec<Vec<f32>> = (0..10u8).map(|n| vec![f32::from(n) / 10.0]).collect();
        let top = top_k(&[1.0], candidates.iter().map(Vec::as_slice), 2);
        assert_eq!(
            top.iter().map(|(index, _)| *index).collect::<Vec<_>>(),
            vec![9, 8]
        );
    }
}
