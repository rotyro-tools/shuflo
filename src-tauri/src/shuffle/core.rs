use std::ops::Range;

use rand::seq::SliceRandom;
use rand::Rng;

/// Shuffles in place so every order is equally likely (Fisher–Yates).
pub fn fisher_yates<T, R: Rng + ?Sized>(items: &mut [T], rng: &mut R) {
    items.shuffle(rng);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    /// Replace the whole playlist with this slice.
    Replace(Range<usize>),
    /// Append this slice to the end.
    Append(Range<usize>),
}

/// Splits writing `n` items into one replace followed by appends of at most `chunk` items.
pub fn write_plan(n: usize, chunk: usize) -> Result<Vec<WriteOp>, String> {
    if n == 0 {
        return Err("The playlist is empty.".to_string());
    }
    let chunk = chunk.max(1);
    let mut ops = vec![WriteOp::Replace(0..n.min(chunk))];
    let mut start = chunk;
    while start < n {
        ops.push(WriteOp::Append(start..(start + chunk).min(n)));
        start += chunk;
    }
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rand::rngs::StdRng;
    use rand::SeedableRng;

    use super::*;

    #[test]
    fn fisher_yates_returns_a_permutation() {
        let mut rng = StdRng::seed_from_u64(7);
        for n in [0usize, 1, 2, 1_000] {
            let original: Vec<usize> = (0..n).collect();
            let mut shuffled = original.clone();
            fisher_yates(&mut shuffled, &mut rng);
            let mut sorted = shuffled.clone();
            sorted.sort_unstable();
            assert_eq!(sorted, original, "n = {n}");
        }
    }

    #[test]
    fn fisher_yates_keeps_duplicates() {
        let mut rng = StdRng::seed_from_u64(1);
        let mut items = vec!["a", "a", "b", "b", "b"];
        fisher_yates(&mut items, &mut rng);
        assert_eq!(items.iter().filter(|s| **s == "a").count(), 2);
        assert_eq!(items.iter().filter(|s| **s == "b").count(), 3);
    }

    #[test]
    fn fisher_yates_is_uniform_over_three_items() {
        // 60,000 shuffles of 3 items: each of the 6 orders should come up ~10,000 times.
        // Chi-square with 5 degrees of freedom; 20.515 is the p = 0.001 critical value.
        const RUNS: usize = 60_000;
        let mut rng = StdRng::seed_from_u64(42);
        let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
        for _ in 0..RUNS {
            let mut items = [0u8, 1, 2];
            fisher_yates(&mut items, &mut rng);
            *counts.entry(items).or_default() += 1;
        }
        assert_eq!(counts.len(), 6);
        let expected = RUNS as f64 / 6.0;
        let chi_square: f64 = counts
            .values()
            .map(|&observed| (observed as f64 - expected).powi(2) / expected)
            .sum();
        assert!(chi_square < 20.515, "chi-square = {chi_square}");
    }

    #[test]
    fn write_plan_chunks_into_one_replace_and_appends() {
        let plan = write_plan(1_000, 100).unwrap();
        assert_eq!(plan.len(), 10);
        assert_eq!(plan[0], WriteOp::Replace(0..100));
        assert_eq!(plan[1], WriteOp::Append(100..200));
        assert_eq!(plan[9], WriteOp::Append(900..1_000));

        assert_eq!(
            write_plan(100, 100).unwrap(),
            vec![WriteOp::Replace(0..100)]
        );
        assert_eq!(
            write_plan(101, 100).unwrap(),
            vec![WriteOp::Replace(0..100), WriteOp::Append(100..101)]
        );
        assert_eq!(write_plan(3, 100).unwrap(), vec![WriteOp::Replace(0..3)]);
    }

    #[test]
    fn write_plan_rejects_an_empty_playlist() {
        assert!(write_plan(0, 100).is_err());
    }
}
