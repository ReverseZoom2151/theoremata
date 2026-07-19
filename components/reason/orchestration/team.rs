//! Concurrent obligation dispatch (plan §6).
//!
//! Independent obligations are processed across OS threads. `Store` holds a
//! single non-`Sync` SQLite connection, so each worker opens its OWN connection
//! to the same database file — WAL mode makes concurrent readers/writers safe.
//! Because rusqlite's default `busy_timeout` is 0, writers retry on a transient
//! "database is locked" while the WAL writer lock is held by a sibling.

use crate::router::{self, NodeSignals, Route, ToolAvailability};
use anyhow::Result;
use std::path::PathBuf;

/// A worker acts as if every capability is available, so routing yields the
/// concrete action it *would* take; the actual solving is delegated elsewhere.
const TOOLS: ToolAvailability = ToolAvailability {
    python: true,
    lean: true,
    formal_verifier: true,
    mathlib_search: true,
    model: true,
    external_prover: true,
};

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkerOutcome {
    pub node_id: String,
    pub route: String,
    pub ok: bool,
}

pub struct Team {
    pub db_path: PathBuf,
    pub max_workers: usize,
}

impl Team {
    /// Dispatch the given (independent) obligations across up to `max_workers`
    /// threads, each with its own DB connection. Every node gets an outcome and
    /// a durable attempt + dispatch-evidence row.
    pub fn process_batch(
        &self,
        project_id: &str,
        node_ids: &[String],
    ) -> Result<Vec<WorkerOutcome>> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        // Bound concurrency and split the work into that many chunks.
        let workers = self.max_workers.max(1).min(node_ids.len());
        let chunk_size = node_ids.len().div_ceil(workers);
        let chunks: Vec<Vec<String>> = node_ids
            .chunks(chunk_size)
            .map(<[String]>::to_vec)
            .collect();

        let db_path = &self.db_path;
        let all = std::thread::scope(|scope| -> Result<Vec<WorkerOutcome>> {
            let handles: Vec<_> = chunks
                .into_iter()
                .map(|chunk| scope.spawn(move || worker(db_path, project_id, chunk)))
                .collect();
            let mut out = Vec::new();
            for handle in handles {
                let part = handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("worker thread panicked"))??;
                out.extend(part);
            }
            Ok(out)
        })?;
        Ok(all)
    }
}

/// One worker thread: its own connection, its own chunk of node ids.
fn worker(db_path: &PathBuf, project_id: &str, chunk: Vec<String>) -> Result<Vec<WorkerOutcome>> {
    let store = crate::db::Store::open(db_path)?;
    let nodes = store.nodes(project_id)?;
    let mut out = Vec::with_capacity(chunk.len());
    for id in chunk {
        let Some(node) = nodes.iter().find(|n| n.id == id) else {
            out.push(WorkerOutcome {
                node_id: id,
                route: "unknown".into(),
                ok: false,
            });
            continue;
        };
        let signals = NodeSignals {
            has_formal_statement: node.formal_statement.is_some(),
            ..Default::default()
        };
        let route = route_name(router::route(node, &signals, &TOOLS, 5));
        retry_locked(|| {
            store.add_attempt(
                project_id,
                Some(&node.id),
                None,
                "worker",
                &serde_json::json!({ "route": route }),
                &serde_json::json!({ "dispatched": true }),
                true,
            )?;
            store.add_evidence(
                project_id,
                &node.id,
                "dispatch",
                "worker",
                &route,
                serde_json::json!({ "route": route }),
            )
        })?;
        out.push(WorkerOutcome {
            node_id: node.id.clone(),
            route,
            ok: true,
        });
    }
    Ok(out)
}

fn route_name(route: Route) -> String {
    serde_json::to_value(route)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{route:?}"))
}

/// Retry a DB write while the WAL writer lock is contended (busy_timeout is 0).
fn retry_locked<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
    let mut last = None;
    for _ in 0..200 {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                let msg = e.to_string().to_lowercase();
                if msg.contains("locked") || msg.contains("busy") {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    last = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("database stayed locked")))
}

/// The scheduler's parallel batches (independent, open, untainted nodes) — feed
/// each batch to [`Team::process_batch`].
pub fn parallel_batches(store: &crate::db::Store, project_id: &str) -> Result<Vec<Vec<String>>> {
    let nodes = store.nodes(project_id)?;
    let edges = store.edges(project_id)?;
    Ok(crate::scheduler::plan(&nodes, &edges).parallel_batches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NodeKind;
    use tempfile::TempDir;

    #[test]
    fn dispatches_concurrently_with_per_thread_connections() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("test.db");
        let store = crate::db::Store::open(&db).unwrap();
        let project = store.create_project("p", "t").unwrap();
        let ids: Vec<String> = (0..8)
            .map(|i| {
                store
                    .add_node(
                        &project.id,
                        NodeKind::Obligation,
                        &format!("o{i}"),
                        "s",
                        "test",
                    )
                    .unwrap()
                    .id
            })
            .collect();
        drop(store); // release the writer before the concurrent workers run

        let team = Team {
            db_path: db.clone(),
            max_workers: 4,
        };
        let outcomes = team.process_batch(&project.id, &ids).unwrap();
        assert_eq!(outcomes.len(), ids.len());
        assert!(outcomes.iter().all(|o| o.ok));
        // fresh obligations with no prior falsification route to falsify first.
        assert!(outcomes.iter().all(|o| o.route == "falsify"));

        // durable rows survived the concurrent writes.
        let store = crate::db::Store::open(&db).unwrap();
        let attempts = store.attempts(&project.id, 100).unwrap();
        assert_eq!(
            attempts.iter().filter(|a| a.actor == "worker").count(),
            ids.len()
        );
    }

    #[test]
    fn empty_batch_is_a_noop() {
        let dir = TempDir::new().unwrap();
        let db = dir.path().join("t.db");
        crate::db::Store::open(&db).unwrap();
        let team = Team {
            db_path: db,
            max_workers: 4,
        };
        assert!(team.process_batch("nope", &[]).unwrap().is_empty());
    }
}
