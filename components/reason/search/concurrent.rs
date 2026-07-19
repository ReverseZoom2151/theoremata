//! **Opt-in true concurrency** for the search fan-outs (the A1 gap:
//! `docs/agentic-patterns-mining/A1-chaining-routing-parallelization.md`).
//!
//! Theoremata's parallel-shaped work — portfolio proving across formal systems
//! ([`super::super::proving::portfolio`]), the multi-alpha accumulative union
//! ([`super::hybrid_search::multi_alpha_union`]), best-of-N sampling — is run
//! **sequentially by design** so results are deterministic and reproducible.
//! That determinism is a feature, not an accident, and this module does **not**
//! take it away. It adds an *opt-in* execution mode that runs those independent
//! branches on real OS threads to cut wall-clock latency, while returning a
//! result that is **byte-identical** to the sequential result — same values, same
//! order, same tie-breaks. Only the wall-clock changes.
//!
//! ## What is preserved
//!
//! * **Result order.** [`collect_all`] / [`run_concurrent`] return results in
//!   **task-index order**, never completion order. Task `i`'s output is always at
//!   position `i`, exactly as a sequential `map` would place it.
//! * **Lowest-index tie-break.** [`first_success`] returns the **lowest-index**
//!   succeeding task even when a higher-index task finishes first — matching a
//!   sequential "return the first success" loop. It never lets a race decide the
//!   winner.
//! * **The sequential default.** [`ConcurrentConfig::enabled`] defaults to
//!   `false`; a disabled config runs on the calling thread with the same control
//!   flow as today. Callers opt *in* to threads; they never opt out of
//!   determinism.
//!
//! ## What is *not* determined
//!
//! Only unobservable scheduling: which worker runs which task, in what real-time
//! order tasks start/finish, and — for [`first_success`] — *which* dominated
//! (higher-index-than-a-known-success) tasks get skipped rather than run. None of
//! that can change the returned value. See the module tests.
//!
//! ## No new dependencies
//!
//! Pure `std`: [`std::thread::scope`], a bounded worker pool keyed off an
//! [`AtomicUsize`] work counter, and [`std::panic::catch_unwind`] so a panicking
//! task is isolated and never poisons its siblings or a shared lock. Tasks are
//! deliberately `'static`: callers cannot accidentally move a borrowed database
//! connection or model provider into a worker merely because the pool uses scoped
//! threads. Use [`run_owned`] when the fan-out starts from owned input values.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::thread;

/// A boxed, sendable proof attempt / alpha pass / best-of-N candidate. `FnOnce`
/// because each task runs exactly once; `Send + 'static` so it can cross to a
/// worker thread without borrowing caller-owned state such as `Store` or a model
/// provider.
pub type Task<T> = Box<dyn FnOnce() -> T + Send + 'static>;

/// Execution policy for the fan-out.
///
/// The default is **sequential** (`enabled: false`) so existing call sites keep
/// their deterministic-by-construction behavior until a caller explicitly opts in.
#[derive(Debug, Clone, Copy)]
pub struct ConcurrentConfig {
    /// Upper bound on OS threads spawned. Clamped to `[1, task_count]`; `0` is
    /// treated as `1`. Ignored when `enabled` is `false`.
    pub max_threads: usize,
    /// When `false` (the default), tasks run sequentially on the calling thread —
    /// identical control flow and identical result to today's code. When `true`,
    /// independent tasks run on up to `max_threads` scoped threads.
    pub enabled: bool,
}

impl ConcurrentConfig {
    /// The sequential policy: run on the calling thread. Identical to today.
    pub fn sequential() -> Self {
        Self {
            max_threads: 1,
            enabled: false,
        }
    }

    /// An enabled policy capped at `max_threads` (clamped to at least `1`).
    pub fn with_threads(max_threads: usize) -> Self {
        Self {
            max_threads: max_threads.max(1),
            enabled: true,
        }
    }

    /// Worker count for `n` tasks under this policy: `1` when disabled, else
    /// `max_threads` clamped into `[1, n]` (never more workers than tasks).
    fn workers_for(&self, n: usize) -> usize {
        if !self.enabled {
            return 1;
        }
        self.max_threads.max(1).min(n.max(1))
    }
}

impl Default for ConcurrentConfig {
    /// Sequential by default — the deterministic default is never surrendered
    /// implicitly. A concurrent-capable default thread count is offered by
    /// [`ConcurrentConfig::default_parallelism`] for callers that opt in.
    fn default() -> Self {
        Self::sequential()
    }
}

impl ConcurrentConfig {
    /// A reasonable thread count for callers that opt in without a preference:
    /// the machine's available parallelism (std, no dependency), or `1` if it
    /// cannot be determined. Note this returns a *count*, not an enabled config —
    /// wrap it with [`ConcurrentConfig::with_threads`] to opt in.
    pub fn default_parallelism() -> usize {
        thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }
}

// ---------------------------------------------------------------------------
// collect_all: every task runs; results returned in task-index order
// ---------------------------------------------------------------------------

/// Run every task and return each one's outcome in **task-index order**, isolating
/// panics: a task that panics yields `Err(payload)` in its slot instead of
/// unwinding the pool or poisoning a sibling.
///
/// This is the panic-isolating core. [`run_concurrent`] is the convenience wrapper
/// that unwraps to `Vec<T>` (re-raising any task panic on the calling thread).
///
/// Determinism: position `i` always holds task `i`'s result, regardless of which
/// worker ran it or when it finished. With `enabled: false` this is a plain
/// in-order loop on the calling thread.
pub fn collect_all_results<T: Send>(
    tasks: Vec<Task<T>>,
    cfg: &ConcurrentConfig,
) -> Vec<thread::Result<T>> {
    let n = tasks.len();
    if n == 0 {
        return Vec::new();
    }
    let workers = cfg.workers_for(n);
    if workers <= 1 {
        // Sequential path: run on the calling thread, in index order. Byte-for-byte
        // the behavior of a `for`-loop fan-out — the deterministic default.
        return tasks
            .into_iter()
            .map(|task| catch_unwind(AssertUnwindSafe(task)))
            .collect();
    }

    // Concurrent path: a fixed pool of `workers` scoped threads pulls task indices
    // from a shared counter (natural load-balancing) and writes each result into
    // its own index slot, so the returned order is task-index order — not
    // completion order.
    let slots: Vec<Mutex<Option<Task<T>>>> =
        tasks.into_iter().map(|t| Mutex::new(Some(t))).collect();
    let results: Vec<Mutex<Option<thread::Result<T>>>> =
        (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);

    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                // Each index is pulled by exactly one worker, so this `take` sees
                // `Some` exactly once. The lock is held only to move the boxed
                // task out — never across the task's own execution — so a task
                // panic cannot poison it.
                let task = slots[i]
                    .lock()
                    .expect("slot lock never held across user code")
                    .take()
                    .expect("each task index is taken exactly once");
                let outcome = catch_unwind(AssertUnwindSafe(task));
                *results[i]
                    .lock()
                    .expect("result lock never held across user code") = Some(outcome);
            });
        }
    });

    results
        .into_iter()
        .map(|m| {
            m.into_inner()
                .expect("result lock never poisoned")
                .expect("every task index was filled")
        })
        .collect()
}

/// Run every task and return the results in **task-index order**.
///
/// The opt-in concurrent counterpart to a sequential `tasks.map(|t| t()).collect()`:
/// with `enabled: true` the tasks run on up to `max_threads` scoped threads, but
/// the returned `Vec` is ordered by task index, identical to the sequential result.
///
/// Panic behavior mirrors a sequential fan-out: if a task panics, that panic is
/// re-raised on the calling thread (after all workers have joined). Callers that
/// want to *observe* a per-task panic instead of propagating it should use
/// [`collect_all_results`].
pub fn run_concurrent<T: Send>(tasks: Vec<Task<T>>, cfg: &ConcurrentConfig) -> Vec<T> {
    collect_all_results(tasks, cfg)
        .into_iter()
        .map(|r| r.unwrap_or_else(|payload| std::panic::resume_unwind(payload)))
        .collect()
}

/// Apply one stateless worker to owned inputs, optionally concurrently, and
/// return outputs in **input order**.
///
/// This is the safe adapter for production fan-outs whose preparation phase is
/// not thread-safe. The caller performs generation, database reads/writes, and
/// provider calls before constructing `inputs`; only the owned values cross the
/// worker boundary. The `'static` bounds reject closures that borrow a `Store`,
/// `&dyn ModelProvider`, or other stack-owned service. Shared worker state must
/// itself be `Send + Sync`.
///
/// With [`ConcurrentConfig::sequential`] (and therefore `Default`) this is an
/// ordinary in-order map on the calling thread. Enabling concurrency changes only
/// scheduling: output position `i` still corresponds to input position `i`.
pub fn run_owned<I, O, F>(inputs: Vec<I>, worker: F, cfg: &ConcurrentConfig) -> Vec<O>
where
    I: Send + 'static,
    O: Send,
    F: Fn(I) -> O + Send + Sync + 'static,
{
    let worker = std::sync::Arc::new(worker);
    let tasks = inputs
        .into_iter()
        .map(|input| {
            let worker = std::sync::Arc::clone(&worker);
            Box::new(move || worker(input)) as Task<O>
        })
        .collect();
    run_concurrent(tasks, cfg)
}

// ---------------------------------------------------------------------------
// first_success: race semantics with a deterministic lowest-index winner
// ---------------------------------------------------------------------------

/// Run tasks concurrently and return the **lowest-index** task whose output
/// satisfies `is_success`, as `Some((index, value))`, or `None` if none succeed.
///
/// This is the race variant for portfolio-style fan-out — it returns without
/// waiting for still-running *higher-index* tasks once a success is known — but the
/// winner it returns is **deterministic**: it is exactly the task a sequential
/// "return the first success" loop would return. A later task finishing first can
/// never win over an earlier task that also succeeds.
///
/// How the tie-break is kept while still cancelling work: a shared `best` holds the
/// lowest success index seen so far. A task index `i` is *skipped* (cancelled,
/// never started) only when `i` is strictly greater than a known success — such a
/// task can never be the lowest-index success, so skipping it cannot change the
/// result. Every index below the eventual winner is therefore always run, and the
/// lowest recorded success is returned. Which dominated indices happen to be
/// skipped vs. run depends on timing, but the returned `(index, value)` does not.
///
/// A panicking task is caught and treated as a non-success (it can never win),
/// keeping one bad task from poisoning the race. With `enabled: false` this is a
/// sequential loop that short-circuits at the first success — identical result.
pub fn first_success<T, F>(
    tasks: Vec<Task<T>>,
    is_success: F,
    cfg: &ConcurrentConfig,
) -> Option<(usize, T)>
where
    T: Send,
    F: Fn(&T) -> bool + Sync,
{
    let n = tasks.len();
    if n == 0 {
        return None;
    }
    let workers = cfg.workers_for(n);
    if workers <= 1 {
        // Sequential path: run in index order, return (and short-circuit on) the
        // first success. This is the reference behavior the concurrent path below
        // reproduces exactly.
        for (i, task) in tasks.into_iter().enumerate() {
            if let Ok(value) = catch_unwind(AssertUnwindSafe(task)) {
                if is_success(&value) {
                    return Some((i, value));
                }
            }
        }
        return None;
    }

    // Concurrent path. `best` = lowest success index found so far (usize::MAX = no
    // success yet). Workers pull indices in increasing order; an index strictly
    // above a known success is dominated and skipped.
    let best = AtomicUsize::new(usize::MAX);
    let slots: Vec<Mutex<Option<Task<T>>>> =
        tasks.into_iter().map(|t| Mutex::new(Some(t))).collect();
    let wins: Vec<Mutex<Option<T>>> = (0..n).map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);

    thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                // Cancellation: a lower index already succeeded ⇒ this task cannot
                // be the lowest-index success. Drop it unrun (its box is freed with
                // the slot). This only ever elides work that cannot affect the
                // result.
                if i > best.load(Ordering::SeqCst) {
                    continue;
                }
                let task = slots[i]
                    .lock()
                    .expect("slot lock never held across user code")
                    .take()
                    .expect("each task index is taken exactly once");
                // A panic is caught and simply is not a success.
                if let Ok(value) = catch_unwind(AssertUnwindSafe(task)) {
                    if is_success(&value) {
                        best.fetch_min(i, Ordering::SeqCst);
                        *wins[i]
                            .lock()
                            .expect("win lock never held across user code") = Some(value);
                    }
                }
            });
        }
    });

    // The lowest index that recorded a success is the deterministic winner. Every
    // index below it was run (skipping only triggers for indices above a success)
    // and did not succeed, so this equals the sequential first-success result.
    for (i, slot) in wins.into_iter().enumerate() {
        let recorded = slot.into_inner().expect("win lock never poisoned");
        if let Some(value) = recorded {
            return Some((i, value));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::sync::Arc;

    /// Build a `Vec<Task<T>>` from a list of closures.
    fn tasks_from<T: Send + 'static>(
        fns: Vec<Box<dyn FnOnce() -> T + Send>>,
    ) -> Vec<Task<T>> {
        fns
    }

    #[test]
    fn collect_all_returns_task_index_order_matching_sequential() {
        // Tasks whose *completion* order (if concurrent) would differ from index
        // order: later indices are cheaper. The result must still be index-ordered.
        let make = || -> Vec<Task<usize>> {
            (0..8usize)
                .map(|i| {
                    let b: Task<usize> = Box::new(move || i * i);
                    b
                })
                .collect()
        };
        let expected: Vec<usize> = (0..8usize).map(|i| i * i).collect();

        let seq = run_concurrent(make(), &ConcurrentConfig::sequential());
        let par = run_concurrent(make(), &ConcurrentConfig::with_threads(4));

        assert_eq!(seq, expected, "sequential is a plain in-order map");
        assert_eq!(par, expected, "concurrent preserves task-index order");
        assert_eq!(seq, par, "concurrent result == sequential result");
    }

    #[test]
    fn run_owned_matches_sequential_and_preserves_input_order() {
        let inputs: Vec<String> = (0..12).map(|i| format!("candidate-{i}")).collect();
        let worker = |input: String| format!("verified:{input}");

        let sequential = run_owned(inputs.clone(), worker, &ConcurrentConfig::default());
        let parallel = run_owned(inputs, worker, &ConcurrentConfig::with_threads(4));

        let expected: Vec<String> = (0..12).map(|i| format!("verified:candidate-{i}")).collect();
        assert_eq!(sequential, expected);
        assert_eq!(parallel, expected);
        assert_eq!(parallel, sequential);
    }

    #[test]
    fn enabled_false_matches_enabled_true_for_collect_all() {
        let make = || -> Vec<Task<String>> {
            (0..16usize)
                .map(|i| {
                    let b: Task<String> = Box::new(move || format!("r{i}"));
                    b
                })
                .collect()
        };
        let off = run_concurrent(make(), &ConcurrentConfig { max_threads: 8, enabled: false });
        let on = run_concurrent(make(), &ConcurrentConfig { max_threads: 8, enabled: true });
        assert_eq!(off, on);
    }

    #[test]
    fn collect_all_is_deterministic_across_runs() {
        let make = || -> Vec<Task<u64>> {
            (0..32u64)
                .map(|i| {
                    let b: Task<u64> = Box::new(move || i.wrapping_mul(2654435761));
                    b
                })
                .collect()
        };
        let cfg = ConcurrentConfig::with_threads(6);
        let a = run_concurrent(make(), &cfg);
        let b = run_concurrent(make(), &cfg);
        let c = run_concurrent(make(), &cfg);
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn first_success_picks_lowest_index_even_when_a_later_task_finishes_first() {
        // Force OUT-OF-ORDER completion: task 0 blocks until task 2 has finished,
        // so task 2 records its success strictly before task 0 does. Both succeed;
        // task 1 fails. The winner MUST be index 0 (the sequential result), never
        // the task that happened to finish first.
        let (tx, rx) = mpsc::channel::<()>();
        let rx = Arc::new(Mutex::new(rx));

        let rx0 = Arc::clone(&rx);
        let t0: Task<i64> = Box::new(move || {
            // Block until task 2 signals it has completed.
            rx0.lock().unwrap().recv().expect("task 2 signals first");
            0 // success (>= 0)
        });
        let t1: Task<i64> = Box::new(|| -1); // failure (< 0)
        let t2: Task<i64> = Box::new(move || {
            let v = 2; // success
            tx.send(()).expect("unblock task 0 after finishing"); // let task 0 proceed
            v
        });

        let cfg = ConcurrentConfig::with_threads(3); // all three run at once
        let got = first_success(tasks_from(vec![t0, t1, t2]), |v| *v >= 0, &cfg);
        assert_eq!(got, Some((0, 0)), "lowest-index success wins the race");
    }

    #[test]
    fn first_success_matches_sequential_for_enabled_false() {
        let make = || -> Vec<Task<i32>> {
            vec![
                Box::new(|| -1),
                Box::new(|| -1),
                Box::new(|| 7), // first success is index 2
                Box::new(|| 9),
            ]
        };
        let off = first_success(make(), |v| *v >= 0, &ConcurrentConfig::sequential());
        let on = first_success(make(), |v| *v >= 0, &ConcurrentConfig::with_threads(4));
        assert_eq!(off, Some((2, 7)));
        assert_eq!(on, Some((2, 7)));
        assert_eq!(off, on);
    }

    #[test]
    fn first_success_none_when_nothing_succeeds() {
        let make = || -> Vec<Task<i32>> {
            (0..5).map(|_| {
                let b: Task<i32> = Box::new(|| -1);
                b
            }).collect()
        };
        assert_eq!(first_success(make(), |v| *v >= 0, &ConcurrentConfig::sequential()), None);
        assert_eq!(first_success(make(), |v| *v >= 0, &ConcurrentConfig::with_threads(4)), None);
    }

    #[test]
    fn a_panicking_task_is_isolated_and_does_not_poison_the_others() {
        // Task 2 panics; every other task must still return its value in order.
        let tasks: Vec<Task<i32>> = vec![
            Box::new(|| 10),
            Box::new(|| 11),
            Box::new(|| panic!("boom in task 2")),
            Box::new(|| 13),
            Box::new(|| 14),
        ];
        let out = collect_all_results(tasks, &ConcurrentConfig::with_threads(4));
        assert_eq!(out.len(), 5);
        assert_eq!(*out[0].as_ref().unwrap(), 10);
        assert_eq!(*out[1].as_ref().unwrap(), 11);
        assert!(out[2].is_err(), "the panicking task is isolated as Err");
        assert_eq!(*out[3].as_ref().unwrap(), 13);
        assert_eq!(*out[4].as_ref().unwrap(), 14);
    }

    #[test]
    fn a_panicking_task_does_not_win_first_success() {
        // Index 0 panics, index 1 succeeds ⇒ the panic is a non-success and index 1
        // wins, both sequentially and concurrently.
        let make = || -> Vec<Task<i32>> {
            vec![
                Box::new(|| panic!("task 0 explodes")),
                Box::new(|| 1),
                Box::new(|| 2),
            ]
        };
        assert_eq!(
            first_success(make(), |v| *v >= 0, &ConcurrentConfig::sequential()),
            Some((1, 1))
        );
        assert_eq!(
            first_success(make(), |v| *v >= 0, &ConcurrentConfig::with_threads(3)),
            Some((1, 1))
        );
    }

    #[test]
    fn empty_task_list_is_handled() {
        let empty: Vec<Task<i32>> = Vec::new();
        assert!(run_concurrent(empty, &ConcurrentConfig::with_threads(4)).is_empty());
        let empty2: Vec<Task<i32>> = Vec::new();
        assert_eq!(first_success(empty2, |_| true, &ConcurrentConfig::with_threads(4)), None);
    }

    #[test]
    fn respects_thread_cap_and_more_threads_than_tasks() {
        // Fewer tasks than threads must not spawn idle-index workers or panic.
        let tasks: Vec<Task<usize>> = (0..3usize)
            .map(|i| {
                let b: Task<usize> = Box::new(move || i);
                b
            })
            .collect();
        let out = run_concurrent(tasks, &ConcurrentConfig::with_threads(64));
        assert_eq!(out, vec![0, 1, 2]);
    }
}
