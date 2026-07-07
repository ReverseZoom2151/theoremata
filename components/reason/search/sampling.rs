//! Best-of-N selection with the verifier as the selector (plan §14).
//!
//! Sample up to `N` candidates and accept the first that passes a predicate.
//! When the acceptance predicate is a *perfect* verifier — the Lean compiler
//! plus the axiom gate — best-of-N has no reward-hacking ceiling: a candidate is
//! accepted only if it genuinely checks. With independent per-sample success
//! probability `p`, the chance at least one of `N` samples succeeds is
//! `P(success) = 1 - (1 - p)^N`, so cheap parallel sampling against a ground
//! truth is unusually powerful here (e.g. `p = 0.3, N = 10` → ~0.97).

use anyhow::Result;

/// A selected candidate together with how it was reached.
#[derive(Debug)]
pub struct Sampled<T> {
    /// The candidate value.
    pub value: T,
    /// Whether it satisfied the acceptance predicate. When `false`, this is the
    /// last successfully generated candidate, offered as a fallback.
    pub accepted: bool,
    /// The generation index (`0..n`) that produced `value`.
    pub index: usize,
    /// How many generation calls were made in total (including skipped errors).
    pub attempts: usize,
}

/// Generate up to `n` candidates and return the first accepted one.
///
/// `generate(i)` is called for `i` in `0..n`. The first candidate for which
/// `accept(&candidate)` holds is returned immediately (early stop). If none are
/// accepted, the *last* successfully generated candidate is returned with
/// `accepted: false` so the caller can still fall back to it. A `generate`
/// error at some index is skipped (still counted toward `attempts`) and
/// generation continues. Returns `Ok(None)` only when `n == 0` or every
/// `generate` call errored (no candidate was ever produced).
pub fn best_of_n<T, G, A>(n: usize, mut generate: G, accept: A) -> Result<Option<Sampled<T>>>
where
    G: FnMut(usize) -> Result<T>,
    A: Fn(&T) -> bool,
{
    let mut attempts = 0usize;
    let mut last: Option<(usize, T)> = None;

    for i in 0..n {
        attempts += 1;
        let candidate = match generate(i) {
            Ok(c) => c,
            Err(_) => continue, // skip a failed generation, keep sampling
        };
        if accept(&candidate) {
            return Ok(Some(Sampled {
                value: candidate,
                accepted: true,
                index: i,
                attempts,
            }));
        }
        last = Some((i, candidate));
    }

    Ok(last.map(|(index, value)| Sampled {
        value,
        accepted: false,
        index,
        attempts,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::cell::Cell;

    #[test]
    fn first_accepted_wins_and_stops_early() {
        // Accept the value 2; candidates are just their index.
        let calls = Cell::new(0usize);
        let result = best_of_n(
            10,
            |i| {
                calls.set(calls.get() + 1);
                Ok(i)
            },
            |v| *v == 2,
        )
        .unwrap()
        .unwrap();

        assert!(result.accepted);
        assert_eq!(result.value, 2);
        assert_eq!(result.index, 2);
        assert_eq!(result.attempts, 3);
        // Early stop: generate was not called past the accepted index.
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn none_accepted_returns_last_as_fallback() {
        let result = best_of_n(4, |i| Ok(i), |_| false).unwrap().unwrap();
        assert!(!result.accepted);
        assert_eq!(result.value, 3);
        assert_eq!(result.index, 3);
        assert_eq!(result.attempts, 4);
    }

    #[test]
    fn generate_errors_are_skipped_and_a_later_good_one_is_found() {
        // Indices 0 and 1 error; index 2 is the first real candidate and passes.
        let result = best_of_n(
            5,
            |i| {
                if i < 2 {
                    Err(anyhow!("transient generation failure at {i}"))
                } else {
                    Ok(format!("cand{i}"))
                }
            },
            |v| v == "cand2",
        )
        .unwrap()
        .unwrap();

        assert!(result.accepted);
        assert_eq!(result.value, "cand2");
        assert_eq!(result.index, 2);
        // Two skipped errors + the accepted third call.
        assert_eq!(result.attempts, 3);
    }

    #[test]
    fn all_errors_returns_none() {
        let result: Option<Sampled<u32>> =
            best_of_n(3, |i| Err(anyhow!("boom {i}")), |_| true).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn zero_n_returns_none() {
        let result: Option<Sampled<u32>> = best_of_n(0, |i| Ok(i as u32), |_| true).unwrap();
        assert!(result.is_none());
    }
}
