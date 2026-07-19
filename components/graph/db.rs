use crate::model::*;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::json;
use sha2::{Digest, Sha256};

/// Lowercase hex of a byte slice. sha2 0.11's digest output no longer implements
/// `LowerHex`, so we format the bytes explicitly.
fn hex_lower(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write as _;
    let bytes = bytes.as_ref();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};
use uuid::Uuid;

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening database {}", path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL;")?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS projects (
              id TEXT PRIMARY KEY, name TEXT NOT NULL, theorem TEXT NOT NULL,
              created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS nodes (
              id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              kind TEXT NOT NULL, status TEXT NOT NULL, title TEXT NOT NULL, statement TEXT NOT NULL,
              formal_statement TEXT, provenance TEXT NOT NULL, content_hash TEXT NOT NULL,
              tainted INTEGER NOT NULL DEFAULT 0,
              tier TEXT NOT NULL DEFAULT 'spine', parent_id TEXT,
              strategy_hint TEXT, suggested_lemmas TEXT NOT NULL DEFAULT '[]',
              lean_decls TEXT NOT NULL DEFAULT '[]',
              stmt_formalized INTEGER NOT NULL DEFAULT 0,
              proof_done INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS edges (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              source_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
              target_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
              kind TEXT NOT NULL, evidence_strength TEXT NOT NULL DEFAULT 'numeric_screen',
              dep_scope TEXT NOT NULL DEFAULT 'statement',
              created_at TEXT NOT NULL,
              UNIQUE(source_id, target_id, kind)
            );
            CREATE TABLE IF NOT EXISTS runs (
              id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              workflow TEXT NOT NULL, state TEXT NOT NULL, current_step TEXT,
              iteration INTEGER NOT NULL DEFAULT 0, started_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS attempts (
              id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              node_id TEXT REFERENCES nodes(id) ON DELETE CASCADE, run_id TEXT REFERENCES runs(id),
              actor TEXT NOT NULL, input TEXT NOT NULL, output TEXT NOT NULL,
              success INTEGER NOT NULL, created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS evidence (
              id TEXT PRIMARY KEY, project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              node_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
              evidence_type TEXT NOT NULL, source TEXT NOT NULL, verdict TEXT NOT NULL,
              payload TEXT NOT NULL, created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS events (
              id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
              run_id TEXT REFERENCES runs(id) ON DELETE SET NULL, event_type TEXT NOT NULL,
              actor TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              role TEXT NOT NULL, content TEXT NOT NULL, metadata TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS proposals (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              action TEXT NOT NULL, status TEXT NOT NULL, proposed_by TEXT NOT NULL,
              resolution_note TEXT, created_at TEXT NOT NULL, resolved_at TEXT
            );
            CREATE TABLE IF NOT EXISTS lemmas (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              name TEXT NOT NULL, statement TEXT NOT NULL,
              source_node_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
              taint INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_nodes_project ON nodes(project_id);
            CREATE INDEX IF NOT EXISTS idx_edges_project ON edges(project_id);
            CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, id);
            CREATE INDEX IF NOT EXISTS idx_messages_project ON messages(project_id, id);
            CREATE INDEX IF NOT EXISTS idx_lemmas_project ON lemmas(project_id);
            CREATE TABLE IF NOT EXISTS proof_jobs (
              id TEXT PRIMARY KEY,
              project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
              node_id TEXT REFERENCES nodes(id) ON DELETE SET NULL,
              backend TEXT NOT NULL,
              status TEXT NOT NULL,
              task_json TEXT NOT NULL,
              result_json TEXT,
              external_id TEXT,
              percent_complete REAL NOT NULL DEFAULT 0,
              artifacts_dir TEXT,
              poll_count INTEGER NOT NULL DEFAULT 0,
              submitted_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              completed_at TEXT
            );
            CREATE TABLE IF NOT EXISTS attempt_runs (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              node_id TEXT REFERENCES nodes(id) ON DELETE SET NULL,
              proof_job_id TEXT REFERENCES proof_jobs(id) ON DELETE SET NULL,
              status TEXT NOT NULL,
              artifacts_dir TEXT NOT NULL,
              input_json TEXT NOT NULL,
              output_json TEXT,
              started_at TEXT NOT NULL,
              updated_at TEXT NOT NULL,
              completed_at TEXT,
              duration_ms INTEGER,
              cost REAL
            );
            CREATE INDEX IF NOT EXISTS idx_proof_jobs_project ON proof_jobs(project_id);
            CREATE INDEX IF NOT EXISTS idx_attempt_runs_project ON attempt_runs(project_id);
            -- Growing verified-lemma library (LEGO-Prover pattern): three logical
            -- stores. `library_lemmas` are admitted, verifier-passed skills;
            -- `library_requests` are conjectured open sub-goals (retrieval queries
            -- + the evolver worklist); `library_problems` are target statements
            -- that bias evolution.
            CREATE TABLE IF NOT EXISTS library_lemmas (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              statement TEXT NOT NULL,
              proof TEXT NOT NULL,
              provenance TEXT NOT NULL,
              embedding_key TEXT NOT NULL,
              update_count INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS library_requests (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              subgoal TEXT NOT NULL,
              provenance TEXT NOT NULL,
              solved INTEGER NOT NULL DEFAULT 0,
              update_count INTEGER NOT NULL DEFAULT 0,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS library_problems (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              statement TEXT NOT NULL,
              provenance TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_library_lemmas_project ON library_lemmas(project_id);
            CREATE INDEX IF NOT EXISTS idx_library_requests_project ON library_requests(project_id);
            CREATE INDEX IF NOT EXISTS idx_library_problems_project ON library_problems(project_id);
            -- Global persistent goal cache (AlphaProof "Nexus"): a canonical-goal
            -- keyed proof cache so proven sub-goals are reused ACROSS searches and
            -- runs, not just within one search's transposition table. All keying /
            -- subsumption policy lives in `reason::search::goal_cache::GoalCache`;
            -- this table is pure CRUD.
            CREATE TABLE IF NOT EXISTS goal_cache (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              canonical_key TEXT NOT NULL,
              goal TEXT NOT NULL,
              proof TEXT NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_goal_cache_project_key ON goal_cache(project_id, canonical_key);

            CREATE TABLE IF NOT EXISTS trace_spans (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              run_id TEXT REFERENCES runs(id) ON DELETE SET NULL,
              span_id INTEGER NOT NULL,
              parent INTEGER,
              kind TEXT NOT NULL,
              label TEXT NOT NULL,
              start_seq INTEGER NOT NULL,
              end_seq INTEGER,
              status TEXT NOT NULL,
              detail TEXT,
              failure_class TEXT,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_trace_spans_run ON trace_spans(run_id, start_seq);
            CREATE INDEX IF NOT EXISTS idx_trace_spans_project ON trace_spans(project_id);
            "#,
        )?;
        // Idempotent column additions so databases created before these fields
        // existed are upgraded in place (a fresh table already has them).
        for (column, decl) in [
            ("tier", "TEXT NOT NULL DEFAULT 'spine'"),
            ("parent_id", "TEXT"),
            ("strategy_hint", "TEXT"),
            ("suggested_lemmas", "TEXT NOT NULL DEFAULT '[]'"),
            ("lean_decls", "TEXT NOT NULL DEFAULT '[]'"),
            ("stmt_formalized", "INTEGER NOT NULL DEFAULT 0"),
            ("proof_done", "INTEGER NOT NULL DEFAULT 0"),
            // Three-valued taint (clean|tainted|self_admitted). Additive over
            // the legacy `tainted` bool: existing rows keep their bool, and the
            // reader reconciles a `taint='clean'` default against `tainted=1`.
            ("taint", "TEXT NOT NULL DEFAULT 'clean'"),
        ] {
            if let Err(e) = self
                .conn
                .execute(&format!("ALTER TABLE nodes ADD COLUMN {column} {decl}"), [])
            {
                if !e.to_string().contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }
        for (column, decl) in [
            (
                "evidence_strength",
                "TEXT NOT NULL DEFAULT 'numeric_screen'",
            ),
            ("dep_scope", "TEXT NOT NULL DEFAULT 'statement'"),
        ] {
            if let Err(e) = self
                .conn
                .execute(&format!("ALTER TABLE edges ADD COLUMN {column} {decl}"), [])
            {
                if !e.to_string().contains("duplicate column name") {
                    return Err(e.into());
                }
            }
        }
        Ok(())
    }

    pub fn create_project(&self, name: &str, theorem: &str) -> Result<Project> {
        let now = Utc::now();
        let p = Project {
            id: Uuid::new_v4().to_string(),
            name: name.to_owned(),
            theorem: theorem.to_owned(),
            created_at: now,
            updated_at: now,
        };
        self.conn.execute(
            "INSERT INTO projects VALUES (?1,?2,?3,?4,?5)",
            params![p.id, p.name, p.theorem, now.to_rfc3339(), now.to_rfc3339()],
        )?;
        self.event(
            Some(&p.id),
            None,
            "project.created",
            "user",
            serde_json::json!({"name": name}),
        )?;
        Ok(p)
    }

    pub fn list_projects(&self) -> Result<Vec<Project>> {
        let mut st = self.conn.prepare(
            "SELECT id,name,theorem,created_at,updated_at FROM projects ORDER BY updated_at DESC",
        )?;
        let rows = st.query_map([], project_row)?;
        let values = rows.collect::<rusqlite::Result<_>>()?;
        Ok(values)
    }

    pub fn project(&self, id: &str) -> Result<Project> {
        self.conn
            .query_row(
                "SELECT id,name,theorem,created_at,updated_at FROM projects WHERE id=?1",
                [id],
                project_row,
            )
            .optional()?
            .ok_or_else(|| anyhow!("project not found: {id}"))
    }

    pub fn add_node(
        &self,
        project_id: &str,
        kind: NodeKind,
        title: &str,
        statement: &str,
        provenance: &str,
    ) -> Result<Node> {
        self.add_node_detailed(
            project_id,
            kind,
            NodeTier::Spine,
            None,
            title,
            statement,
            None,
            &[],
            provenance,
        )
    }

    /// Create a node with full blueprint metadata: its tier (spine vs
    /// implementation), an optional owning parent, a strategy hint, and
    /// suggested Mathlib lemmas to try — the human convention of annotating
    /// each obligation with its target lemma, which is what a proving agent
    /// needs as a prompt.
    #[allow(clippy::too_many_arguments)]
    pub fn add_node_detailed(
        &self,
        project_id: &str,
        kind: NodeKind,
        tier: NodeTier,
        parent_id: Option<&str>,
        title: &str,
        statement: &str,
        strategy_hint: Option<&str>,
        suggested_lemmas: &[String],
        provenance: &str,
    ) -> Result<Node> {
        self.project(project_id)?;
        let now = Utc::now();
        let hash = hex_lower(Sha256::digest(format!("{kind}|{title}|{statement}")));
        let lemmas_json = serde_json::to_string(suggested_lemmas)?;
        let node = Node {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            kind,
            status: NodeStatus::Proposed,
            title: title.to_owned(),
            statement: statement.to_owned(),
            formal_statement: None,
            provenance: provenance.to_owned(),
            content_hash: hash,
            tainted: false,
            taint: Taint::Clean,
            tier,
            parent_id: parent_id.map(str::to_owned),
            strategy_hint: strategy_hint.map(str::to_owned),
            suggested_lemmas: suggested_lemmas.to_vec(),
            lean_decls: Vec::new(),
            stmt_formalized: false,
            proof_done: false,
            created_at: now,
            updated_at: now,
        };
        self.conn.execute(
            "INSERT INTO nodes(id,project_id,kind,status,title,statement,formal_statement,\
             provenance,content_hash,tainted,tier,parent_id,strategy_hint,suggested_lemmas,\
             lean_decls,created_at,updated_at) \
             VALUES (?1,?2,?3,?4,?5,?6,NULL,?7,?8,0,?9,?10,?11,?12,'[]',?13,?13)",
            params![
                node.id,
                project_id,
                kind.to_string(),
                node.status.to_string(),
                title,
                statement,
                provenance,
                node.content_hash,
                tier.to_string(),
                parent_id,
                strategy_hint,
                lemmas_json,
                now.to_rfc3339()
            ],
        )?;
        self.touch(project_id)?;
        self.event(
            Some(project_id),
            None,
            "node.created",
            provenance,
            serde_json::json!({"node_id":node.id,"kind":kind,"title":title,"tier":tier}),
        )?;
        Ok(node)
    }

    pub fn set_strategy_hint(
        &self,
        project_id: &str,
        node_id: &str,
        hint: Option<&str>,
        actor: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE nodes SET strategy_hint=?1,updated_at=?2 WHERE id=?3 AND project_id=?4",
            params![hint, Utc::now().to_rfc3339(), node_id, project_id],
        )?;
        if changed == 0 {
            return Err(anyhow!("node not found: {node_id}"));
        }
        self.event(
            Some(project_id),
            None,
            "node.hint_set",
            actor,
            json!({ "node_id": node_id }),
        )?;
        Ok(())
    }

    pub fn set_suggested_lemmas(
        &self,
        project_id: &str,
        node_id: &str,
        lemmas: &[String],
        actor: &str,
    ) -> Result<()> {
        let payload = serde_json::to_string(lemmas)?;
        let changed = self.conn.execute(
            "UPDATE nodes SET suggested_lemmas=?1,updated_at=?2 WHERE id=?3 AND project_id=?4",
            params![payload, Utc::now().to_rfc3339(), node_id, project_id],
        )?;
        if changed == 0 {
            return Err(anyhow!("node not found: {node_id}"));
        }
        self.event(
            Some(project_id),
            None,
            "node.lemmas_set",
            actor,
            json!({"node_id": node_id, "count": lemmas.len()}),
        )?;
        Ok(())
    }

    pub fn set_lean_decls(
        &self,
        project_id: &str,
        node_id: &str,
        decls: &[String],
        actor: &str,
    ) -> Result<()> {
        let payload = serde_json::to_string(decls)?;
        let changed = self.conn.execute(
            "UPDATE nodes SET lean_decls=?1,updated_at=?2 WHERE id=?3 AND project_id=?4",
            params![payload, Utc::now().to_rfc3339(), node_id, project_id],
        )?;
        if changed == 0 {
            return Err(anyhow!("node not found: {node_id}"));
        }
        self.event(
            Some(project_id),
            None,
            "node.lean_decls_set",
            actor,
            json!({"node_id": node_id, "count": decls.len()}),
        )?;
        Ok(())
    }

    pub fn set_node_status(
        &self,
        project_id: &str,
        node_id: &str,
        status: NodeStatus,
        actor: &str,
    ) -> Result<()> {
        // Atomic: the status UPDATE, the (conditional) taint recomputation and
        // the `node.status_changed` event commit together, so an observer never
        // sees a new status without its event or with stale taint.
        let tx = self.conn.unchecked_transaction()?;
        let changed = self.conn.execute(
            "UPDATE nodes SET status=?1,updated_at=?2 WHERE id=?3 AND project_id=?4",
            params![
                status.to_string(),
                Utc::now().to_rfc3339(),
                node_id,
                project_id
            ],
        )?;
        if changed == 0 {
            return Err(anyhow!("node not found: {node_id}"));
        }
        if matches!(status, NodeStatus::Rejected | NodeStatus::Blocked) {
            self.recompute_taint(project_id)?;
        }
        self.event(
            Some(project_id),
            None,
            "node.status_changed",
            actor,
            serde_json::json!({"node_id":node_id,"status":status}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_formal_statement(
        &self,
        project_id: &str,
        node_id: &str,
        formal: &str,
        actor: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE nodes SET formal_statement=?1,updated_at=?2 WHERE id=?3 AND project_id=?4",
            params![formal, Utc::now().to_rfc3339(), node_id, project_id],
        )?;
        if changed == 0 {
            return Err(anyhow!("node not found: {node_id}"));
        }
        self.event(
            Some(project_id),
            None,
            "node.formalized",
            actor,
            serde_json::json!({"node_id":node_id}),
        )?;
        Ok(())
    }

    /// Set the leanblueprint `\leanok` verification flags for a node: whether
    /// its *statement* is formalised and whether its *proof* is complete. These
    /// are independent — a node is commonly statement-formalised with an open
    /// proof.
    pub fn set_verification_flags(
        &self,
        project_id: &str,
        node_id: &str,
        stmt_formalized: bool,
        proof_done: bool,
        actor: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE nodes SET stmt_formalized=?1,proof_done=?2,updated_at=?3 \
             WHERE id=?4 AND project_id=?5",
            params![
                stmt_formalized as i64,
                proof_done as i64,
                Utc::now().to_rfc3339(),
                node_id,
                project_id
            ],
        )?;
        if changed == 0 {
            return Err(anyhow!("node not found: {node_id}"));
        }
        self.event(
            Some(project_id),
            None,
            "node.leanok_set",
            actor,
            json!({"node_id":node_id,"stmt_formalized":stmt_formalized,"proof_done":proof_done}),
        )?;
        Ok(())
    }

    /// Add a dependency edge with a statement-scoped `\uses` (the conservative
    /// default). Prefer [`Store::add_edge_scoped`] when the blueprint
    /// distinguishes statement-deps from proof-deps.
    pub fn add_edge(
        &self,
        project_id: &str,
        source: &str,
        target: &str,
        kind: EdgeKind,
    ) -> Result<()> {
        self.add_edge_scoped(project_id, source, target, kind, DepScope::Statement)
    }

    /// Add a dependency edge tagged with its leanblueprint `\uses` scope
    /// (statement / proof / both). Cycle-checked; taint recomputed.
    pub fn add_edge_scoped(
        &self,
        project_id: &str,
        source: &str,
        target: &str,
        kind: EdgeKind,
        dep_scope: DepScope,
    ) -> Result<()> {
        // Atomic: the insert, the acyclicity check, the cycle-rollback delete,
        // the taint recomputation and the event write all commit together. On
        // any early return the transaction is dropped and rolled back, so a
        // failure mid-sequence leaves no partial edge row, and a concurrent
        // insert cannot slip a cycle in between the check and the write.
        let tx = self.conn.unchecked_transaction()?;
        self.add_edge_scoped_inner(project_id, source, target, kind, dep_scope)?;
        tx.commit()?;
        Ok(())
    }

    /// Transaction-free core of [`Store::add_edge_scoped`]. Every statement runs
    /// on `self.conn`, so it participates in whatever transaction the caller has
    /// already opened (e.g. [`Store::merge_nodes`]). It never opens its own
    /// transaction — call it only from inside one.
    fn add_edge_scoped_inner(
        &self,
        project_id: &str,
        source: &str,
        target: &str,
        kind: EdgeKind,
        dep_scope: DepScope,
    ) -> Result<()> {
        if source == target {
            return Err(anyhow!("self edges are not allowed"));
        }
        self.conn.execute(
            "INSERT INTO edges(project_id,source_id,target_id,kind,evidence_strength,dep_scope,created_at) \
             VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                project_id,
                source,
                target,
                kind.to_string(),
                EdgeStrength::NumericScreen.to_string(),
                dep_scope.to_string(),
                Utc::now().to_rfc3339()
            ],
        ).context("adding edge (nodes must belong to the project and edge must be unique)")?;
        if self.has_cycle(project_id)? {
            self.conn.execute(
                "DELETE FROM edges WHERE project_id=?1 AND source_id=?2 AND target_id=?3 AND kind=?4",
                params![project_id, source, target, kind.to_string()],
            )?;
            return Err(anyhow!("edge would create a dependency cycle"));
        }
        self.recompute_taint(project_id)?;
        self.event(
            Some(project_id),
            None,
            "edge.created",
            "user",
            serde_json::json!({"source":source,"target":target,"kind":kind,"dep_scope":dep_scope.to_string()}),
        )?;
        Ok(())
    }

    /// Widen an existing edge's `\uses` scope, merging the new scope into the
    /// current one (statement + proof ⇒ both). No-op if the edge is absent.
    pub fn widen_edge_scope(
        &self,
        project_id: &str,
        source: &str,
        target: &str,
        kind: EdgeKind,
        scope: DepScope,
    ) -> Result<()> {
        let current: Option<String> = self
            .conn
            .query_row(
                "SELECT dep_scope FROM edges WHERE project_id=?1 AND source_id=?2 \
                 AND target_id=?3 AND kind=?4",
                params![project_id, source, target, kind.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(current) = current else {
            return Ok(());
        };
        let merged = current
            .parse::<DepScope>()
            .unwrap_or(DepScope::Statement)
            .merge(scope);
        self.conn.execute(
            "UPDATE edges SET dep_scope=?1 WHERE project_id=?2 AND source_id=?3 \
             AND target_id=?4 AND kind=?5",
            params![
                merged.to_string(),
                project_id,
                source,
                target,
                kind.to_string()
            ],
        )?;
        Ok(())
    }

    /// Graph-of-Thoughts merge (plan §14): create a new node that depends on
    /// several independently-proven parents — prove A and B separately, then
    /// merge into a child that needs both. Adds the node plus a DependsOn edge
    /// from the child to each parent (cycle-checked by `add_edge`).
    pub fn merge_nodes(
        &self,
        project_id: &str,
        kind: NodeKind,
        title: &str,
        statement: &str,
        parents: &[String],
        provenance: &str,
    ) -> Result<Node> {
        // Atomic: the merged node, every DependsOn edge to its parents (each
        // cycle-checked) and the `nodes.merged` event commit together. A failure
        // partway — e.g. a parent that would form a cycle — rolls the whole
        // merge back, leaving neither a dangling node nor partial edges. The
        // edges use the transaction-free `add_edge_scoped_inner` so they join
        // this transaction instead of opening (illegally nested) ones.
        let tx = self.conn.unchecked_transaction()?;
        let node = self.add_node(project_id, kind, title, statement, provenance)?;
        for parent in parents {
            self.add_edge_scoped_inner(
                project_id,
                &node.id,
                parent,
                EdgeKind::DependsOn,
                DepScope::Statement,
            )?;
        }
        self.event(
            Some(project_id),
            None,
            "nodes.merged",
            provenance,
            serde_json::json!({"node_id": node.id, "parents": parents}),
        )?;
        tx.commit()?;
        Ok(node)
    }

    /// Record how strongly an existing edge's dependency is backed
    /// (numeric_screen < prose_proof < lean_checked).
    pub fn set_edge_strength(
        &self,
        project_id: &str,
        source: &str,
        target: &str,
        kind: EdgeKind,
        strength: EdgeStrength,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE edges SET evidence_strength=?1 WHERE project_id=?2 AND source_id=?3 \
             AND target_id=?4 AND kind=?5",
            params![
                strength.to_string(),
                project_id,
                source,
                target,
                kind.to_string()
            ],
        )?;
        if changed == 0 {
            return Err(anyhow!("edge not found"));
        }
        self.event(
            Some(project_id),
            None,
            "edge.strength_set",
            "user",
            json!({"source":source,"target":target,"kind":kind,"strength":strength.to_string()}),
        )?;
        Ok(())
    }

    pub fn nodes(&self, project_id: &str) -> Result<Vec<Node>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,kind,status,title,statement,formal_statement,provenance,
             content_hash,tainted,tier,parent_id,strategy_hint,suggested_lemmas,lean_decls,
             created_at,updated_at,
             stmt_formalized,proof_done,taint
             FROM nodes WHERE project_id=?1 ORDER BY created_at",
        )?;
        let rows = st.query_map([project_id], node_row)?;
        let values = rows.collect::<rusqlite::Result<_>>()?;
        Ok(values)
    }

    pub fn edges(&self, project_id: &str) -> Result<Vec<Edge>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,source_id,target_id,kind,evidence_strength,dep_scope,created_at FROM edges WHERE project_id=?1 ORDER BY id",
        )?;
        let rows = st.query_map([project_id], edge_row)?;
        let values = rows.collect::<rusqlite::Result<_>>()?;
        Ok(values)
    }

    pub fn events(&self, project_id: &str, limit: usize) -> Result<Vec<Event>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,run_id,event_type,actor,payload,created_at FROM events
             WHERE project_id=?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![project_id, limit as i64], event_row)?;
        let values = rows.collect::<rusqlite::Result<_>>()?;
        Ok(values)
    }

    pub fn add_message(
        &self,
        project_id: &str,
        role: &str,
        content: &str,
        metadata: serde_json::Value,
    ) -> Result<ChatMessage> {
        self.project(project_id)?;
        let now = Utc::now();
        self.conn.execute(
            "INSERT INTO messages(project_id,role,content,metadata,created_at) VALUES (?1,?2,?3,?4,?5)",
            params![project_id,role,content,metadata.to_string(),now.to_rfc3339()],
        )?;
        let id = self.conn.last_insert_rowid();
        self.event(
            Some(project_id),
            None,
            "chat.message",
            role,
            json!({"message_id":id}),
        )?;
        Ok(ChatMessage {
            id,
            project_id: project_id.into(),
            role: role.into(),
            content: content.into(),
            metadata,
            created_at: now,
        })
    }

    pub fn messages(&self, project_id: &str, limit: usize) -> Result<Vec<ChatMessage>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,role,content,metadata,created_at FROM (
               SELECT * FROM messages WHERE project_id=?1 ORDER BY id DESC LIMIT ?2
             ) ORDER BY id",
        )?;
        let rows = st.query_map(params![project_id, limit as i64], |r| {
            let raw: String = r.get(4)?;
            Ok(ChatMessage {
                id: r.get(0)?,
                project_id: r.get(1)?,
                role: r.get(2)?,
                content: r.get(3)?,
                metadata: serde_json::from_str(&raw).unwrap_or_default(),
                created_at: dt(r.get(5)?)?,
            })
        })?;
        let values = rows.collect::<rusqlite::Result<_>>()?;
        Ok(values)
    }

    pub fn add_proposal(
        &self,
        project_id: &str,
        action: serde_json::Value,
        proposed_by: &str,
    ) -> Result<Proposal> {
        let proposal = Proposal {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.into(),
            action,
            status: "pending".into(),
            proposed_by: proposed_by.into(),
            resolution_note: None,
            created_at: Utc::now(),
            resolved_at: None,
        };
        self.conn.execute(
            "INSERT INTO proposals VALUES (?1,?2,?3,?4,?5,NULL,?6,NULL)",
            params![
                proposal.id,
                proposal.project_id,
                proposal.action.to_string(),
                proposal.status,
                proposal.proposed_by,
                proposal.created_at.to_rfc3339()
            ],
        )?;
        self.event(
            Some(project_id),
            None,
            "proposal.created",
            proposed_by,
            json!({"proposal_id":proposal.id,"action":proposal.action}),
        )?;
        Ok(proposal)
    }

    pub fn proposals(&self, project_id: &str, pending_only: bool) -> Result<Vec<Proposal>> {
        let sql = if pending_only {
            "SELECT id,project_id,action,status,proposed_by,resolution_note,created_at,resolved_at
             FROM proposals WHERE project_id=?1 AND status='pending' ORDER BY created_at"
        } else {
            "SELECT id,project_id,action,status,proposed_by,resolution_note,created_at,resolved_at
             FROM proposals WHERE project_id=?1 ORDER BY created_at"
        };
        let mut statement = self.conn.prepare(sql)?;
        let rows = statement.query_map([project_id], proposal_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn proposal(&self, project_id: &str, id: &str) -> Result<Proposal> {
        self.conn
            .query_row(
                "SELECT id,project_id,action,status,proposed_by,resolution_note,created_at,resolved_at
                 FROM proposals WHERE project_id=?1 AND id=?2",
                params![project_id, id],
                proposal_row,
            )
            .optional()?
            .ok_or_else(|| anyhow!("proposal not found: {id}"))
    }

    pub fn resolve_proposal(
        &self,
        project_id: &str,
        id: &str,
        status: &str,
        note: &str,
    ) -> Result<()> {
        if !matches!(status, "approved" | "rejected") {
            return Err(anyhow!("invalid proposal resolution"));
        }
        let changed = self.conn.execute(
            "UPDATE proposals SET status=?1,resolution_note=?2,resolved_at=?3
             WHERE id=?4 AND project_id=?5 AND status='pending'",
            params![status, note, Utc::now().to_rfc3339(), id, project_id],
        )?;
        if changed == 0 {
            return Err(anyhow!("proposal is absent or already resolved: {id}"));
        }
        self.event(
            Some(project_id),
            None,
            &format!("proposal.{status}"),
            "user",
            json!({"proposal_id":id,"note":note}),
        )?;
        Ok(())
    }

    pub fn export(&self, project_id: &str) -> Result<GraphExport> {
        Ok(GraphExport {
            project: self.project(project_id)?,
            nodes: self.nodes(project_id)?,
            edges: self.edges(project_id)?,
            events: self.events(project_id, 100_000)?,
        })
    }

    pub fn begin_run(&self, project_id: &str, workflow: &str) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO runs VALUES (?1,?2,?3,'running','start',0,?4,?4)",
            params![id, project_id, workflow, now],
        )?;
        self.event(
            Some(project_id),
            Some(&id),
            "run.started",
            "orchestrator",
            serde_json::json!({"workflow":workflow}),
        )?;
        Ok(id)
    }

    pub fn update_run(
        &self,
        project_id: &str,
        run_id: &str,
        state: &str,
        step: &str,
        iteration: u32,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET state=?1,current_step=?2,iteration=?3,updated_at=?4 WHERE id=?5",
            params![state, step, iteration, Utc::now().to_rfc3339(), run_id],
        )?;
        self.event(
            Some(project_id),
            Some(run_id),
            "run.step",
            "orchestrator",
            serde_json::json!({"state":state,"step":step,"iteration":iteration}),
        )?;
        Ok(())
    }

    pub fn add_evidence(
        &self,
        project_id: &str,
        node_id: &str,
        kind: &str,
        source: &str,
        verdict: &str,
        payload: serde_json::Value,
    ) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO evidence VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                project_id,
                node_id,
                kind,
                source,
                verdict,
                payload.to_string(),
                Utc::now().to_rfc3339()
            ],
        )?;
        self.event(
            Some(project_id),
            None,
            "evidence.recorded",
            source,
            serde_json::json!({"node_id":node_id,"evidence_type":kind,"verdict":verdict}),
        )?;
        Ok(id)
    }

    /// Record a solver attempt — a single tool run or model call against a node
    /// — with its input, output, and success. Attempts are the durable record
    /// of failed strategies the retry policy and scheduler reason over.
    #[allow(clippy::too_many_arguments)]
    pub fn add_attempt(
        &self,
        project_id: &str,
        node_id: Option<&str>,
        run_id: Option<&str>,
        actor: &str,
        input: &serde_json::Value,
        output: &serde_json::Value,
        success: bool,
    ) -> Result<Attempt> {
        let now = Utc::now();
        let attempt = Attempt {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            node_id: node_id.map(str::to_owned),
            run_id: run_id.map(str::to_owned),
            actor: actor.to_owned(),
            input: input.clone(),
            output: output.clone(),
            success,
            created_at: now,
        };
        self.conn.execute(
            "INSERT INTO attempts(id,project_id,node_id,run_id,actor,input,output,success,created_at)\
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                attempt.id,
                project_id,
                node_id,
                run_id,
                actor,
                input.to_string(),
                output.to_string(),
                success as i64,
                now.to_rfc3339()
            ],
        )?;
        self.event(
            Some(project_id),
            run_id,
            "attempt.recorded",
            actor,
            json!({"attempt_id":attempt.id,"node_id":node_id,"success":success}),
        )?;
        Ok(attempt)
    }

    pub fn attempts(&self, project_id: &str, limit: usize) -> Result<Vec<Attempt>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,node_id,run_id,actor,input,output,success,created_at
             FROM attempts WHERE project_id=?1 ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![project_id, limit as i64], attempt_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_proof_job(
        &self,
        task: &crate::prover::model::ProofTask,
        backend: &str,
        status: crate::prover::model::ProverJobStatus,
        external_id: Option<&str>,
        artifacts_dir: Option<&Path>,
        percent_complete: f64,
    ) -> Result<crate::prover::model::ProofJob> {
        let now = Utc::now();
        let job = crate::prover::model::ProofJob {
            id: Uuid::new_v4().to_string(),
            project_id: task.project_id.clone(),
            node_id: task.node_id.clone(),
            backend: backend.to_owned(),
            status,
            task: task.clone(),
            result: None,
            external_id: external_id.map(str::to_owned),
            percent_complete,
            artifacts_dir: artifacts_dir.map(Path::to_path_buf),
            poll_count: 0,
            submitted_at: now,
            updated_at: now,
            completed_at: None,
        };
        self.conn.execute(
            "INSERT INTO proof_jobs(id,project_id,node_id,backend,status,task_json,result_json,\
             external_id,percent_complete,artifacts_dir,poll_count,submitted_at,updated_at,completed_at)\
             VALUES (?1,?2,?3,?4,?5,?6,NULL,?7,?8,?9,0,?10,?10,NULL)",
            params![
                job.id,
                job.project_id,
                job.node_id,
                job.backend,
                serde_json::to_string(&job.status)?,
                serde_json::to_string(&job.task)?,
                job.external_id,
                job.percent_complete,
                job.artifacts_dir.as_ref().map(|p| p.to_string_lossy().to_string()),
                now.to_rfc3339(),
            ],
        )?;
        Ok(job)
    }

    pub fn update_proof_job(&self, job: &crate::prover::model::ProofJob) -> Result<()> {
        self.conn.execute(
            "UPDATE proof_jobs SET status=?1, result_json=?2, percent_complete=?3, poll_count=?4,\
             updated_at=?5, completed_at=?6 WHERE id=?7",
            params![
                serde_json::to_string(&job.status)?,
                job.result.as_ref().map(serde_json::to_string).transpose()?,
                job.percent_complete,
                job.poll_count,
                job.updated_at.to_rfc3339(),
                job.completed_at.map(|t| t.to_rfc3339()),
                job.id,
            ],
        )?;
        Ok(())
    }

    pub fn get_proof_job(&self, id: &str) -> Result<Option<crate::prover::model::ProofJob>> {
        self.conn
            .query_row(
                "SELECT id,project_id,node_id,backend,status,task_json,result_json,external_id,\
                 percent_complete,artifacts_dir,poll_count,submitted_at,updated_at,completed_at \
                 FROM proof_jobs WHERE id=?1",
                params![id],
                proof_job_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_proof_jobs(
        &self,
        project_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::prover::model::ProofJob>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,node_id,backend,status,task_json,result_json,external_id,\
             percent_complete,artifacts_dir,poll_count,submitted_at,updated_at,completed_at \
             FROM proof_jobs WHERE project_id=?1 ORDER BY submitted_at DESC LIMIT ?2",
        )?;
        let rows = st.query_map(params![project_id, limit as i64], proof_job_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn create_attempt_run(
        &self,
        project_id: &str,
        node_id: Option<&str>,
        proof_job_id: Option<&str>,
        status: crate::prover::model::AttemptRunStatus,
        artifacts_dir: &Path,
        input: &serde_json::Value,
    ) -> Result<crate::prover::model::AttemptRunRecord> {
        let now = Utc::now();
        let record = crate::prover::model::AttemptRunRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            node_id: node_id.map(str::to_owned),
            proof_job_id: proof_job_id.map(str::to_owned),
            status,
            artifacts_dir: artifacts_dir.to_path_buf(),
            input: input.clone(),
            output: None,
            started_at: now,
            updated_at: now,
            completed_at: None,
            duration_ms: None,
            cost: None,
        };
        self.conn.execute(
            "INSERT INTO attempt_runs(id,project_id,node_id,proof_job_id,status,artifacts_dir,\
             input_json,output_json,started_at,updated_at,completed_at,duration_ms,cost)\
             VALUES (?1,?2,?3,?4,?5,?6,?7,NULL,?8,?8,NULL,NULL,NULL)",
            params![
                record.id,
                project_id,
                node_id,
                proof_job_id,
                serde_json::to_string(&record.status)?,
                artifacts_dir.to_string_lossy().to_string(),
                input.to_string(),
                now.to_rfc3339(),
            ],
        )?;
        Ok(record)
    }

    pub fn update_attempt_run(
        &self,
        record: &crate::prover::model::AttemptRunRecord,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE attempt_runs SET status=?1, output_json=?2, updated_at=?3, completed_at=?4,\
             duration_ms=?5, cost=?6 WHERE id=?7",
            params![
                serde_json::to_string(&record.status)?,
                record
                    .output
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                record.updated_at.to_rfc3339(),
                record.completed_at.map(|t| t.to_rfc3339()),
                record.duration_ms.map(|v| v as i64),
                record.cost,
                record.id,
            ],
        )?;
        Ok(())
    }

    pub fn get_attempt_run(
        &self,
        id: &str,
    ) -> Result<Option<crate::prover::model::AttemptRunRecord>> {
        self.conn
            .query_row(
                "SELECT id,project_id,node_id,proof_job_id,status,artifacts_dir,input_json,\
                 output_json,started_at,updated_at,completed_at,duration_ms,cost \
                 FROM attempt_runs WHERE id=?1",
                params![id],
                attempt_run_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn event(
        &self,
        project_id: Option<&str>,
        run_id: Option<&str>,
        ty: &str,
        actor: &str,
        payload: serde_json::Value,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events(project_id,run_id,event_type,actor,payload,created_at)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                project_id,
                run_id,
                ty,
                actor,
                payload.to_string(),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Persist a finished [`RunTrace`](crate::trace::RunTrace): one row per span,
    /// with each failed span's [`FailureClass`](crate::trace::FailureClass) folded
    /// in. Flushed once at run end. Also mirrors the whole tree onto the event log
    /// under `run.trace`, so the existing observe/replay path keeps working with no
    /// reader change.
    pub fn record_trace(
        &self,
        project_id: &str,
        run_id: &str,
        trace: &crate::trace::RunTrace,
        failures: &std::collections::HashMap<u64, crate::trace::FailureClass>,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        for span in trace.spans() {
            // SpanKind/SpanStatus serialize (serde) to their snake_case tag; they
            // have no Display/as_str, so round-trip through serde_json to a String.
            let kind = serde_json::to_value(span.kind)?
                .as_str()
                .unwrap_or("other")
                .to_owned();
            let status = serde_json::to_value(span.status)?
                .as_str()
                .unwrap_or("open")
                .to_owned();
            let failure_class = failures.get(&span.id).map(|c| c.as_str());
            let id = Uuid::new_v4().to_string();
            let span_id = span.id as i64;
            let parent = span.parent.map(|p| p as i64);
            let start_seq = span.start_seq as i64;
            let end_seq = span.end_seq.map(|e| e as i64);
            self.conn.execute(
                "INSERT INTO trace_spans(id,project_id,run_id,span_id,parent,kind,label,\
                 start_seq,end_seq,status,detail,failure_class,created_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params![
                    id,
                    project_id,
                    run_id,
                    span_id,
                    parent,
                    kind,
                    span.label,
                    start_seq,
                    end_seq,
                    status,
                    span.detail,
                    failure_class,
                    now,
                ],
            )?;
        }
        self.event(
            Some(project_id),
            Some(run_id),
            "run.trace",
            "orchestrator",
            trace.to_tree(),
        )?;
        Ok(())
    }

    fn touch(&self, project_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET updated_at=?1 WHERE id=?2",
            params![Utc::now().to_rfc3339(), project_id],
        )?;
        Ok(())
    }

    fn has_cycle(&self, project_id: &str) -> Result<bool> {
        let edges = self.edges(project_id)?;
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for e in edges {
            adj.entry(e.source_id).or_default().push(e.target_id);
        }
        fn visit(
            n: &str,
            adj: &HashMap<String, Vec<String>>,
            gray: &mut HashSet<String>,
            black: &mut HashSet<String>,
        ) -> bool {
            if gray.contains(n) {
                return true;
            }
            if black.contains(n) {
                return false;
            }
            gray.insert(n.to_owned());
            if adj
                .get(n)
                .is_some_and(|xs| xs.iter().any(|x| visit(x, adj, gray, black)))
            {
                return true;
            }
            gray.remove(n);
            black.insert(n.to_owned());
            false
        }
        let mut gray = HashSet::new();
        let mut black = HashSet::new();
        Ok(adj.keys().any(|n| visit(n, &adj, &mut gray, &mut black)))
    }

    /// Recompute three-valued taint for the whole project and persist it.
    /// Delegates the classification to the executable propagation in
    /// [`crate::taint::propagate`] (rejected/blocked → `Tainted`, explicit gaps
    /// stay `SelfAdmitted`, dependents of either become `Tainted`), then writes
    /// both the three-valued `taint` column and the legacy `tainted` bool so
    /// old call sites keep working.
    fn recompute_taint(&self, project_id: &str) -> Result<()> {
        let nodes = self.nodes(project_id)?;
        let edges = self.edges(project_id)?;
        let taints = crate::taint::propagate(&nodes, &edges);
        for node in &nodes {
            let taint = taints.get(&node.id).copied().unwrap_or(Taint::Clean);
            self.conn.execute(
                "UPDATE nodes SET taint=?1, tainted=?2 WHERE id=?3",
                params![taint.to_string(), taint.is_tainted() as i64, node.id],
            )?;
        }
        Ok(())
    }

    /// Explicitly set a node's three-valued taint — used to mark a node as a
    /// `SelfAdmitted` gap (an admitted `sorry`/obligation the proof leans on).
    /// The mark is sticky: the subsequent propagation preserves `SelfAdmitted`
    /// sources and poisons their dependents as `Tainted`.
    pub fn set_taint(
        &self,
        project_id: &str,
        node_id: &str,
        taint: Taint,
        actor: &str,
    ) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE nodes SET taint=?1, tainted=?2, updated_at=?3 WHERE id=?4 AND project_id=?5",
            params![
                taint.to_string(),
                taint.is_tainted() as i64,
                Utc::now().to_rfc3339(),
                node_id,
                project_id
            ],
        )?;
        if changed == 0 {
            return Err(anyhow!("node not found: {node_id}"));
        }
        self.event(
            Some(project_id),
            None,
            "node.taint_set",
            actor,
            json!({"node_id": node_id, "taint": taint.to_string()}),
        )?;
        self.recompute_taint(project_id)?;
        Ok(())
    }

    /// Transitive DependsOn dependency closure of a node, including the node
    /// itself. `A depends on B` is an edge `source=A, target=B`.
    fn dependency_closure(&self, project_id: &str, root: &str) -> Result<Vec<String>> {
        let edges = self.edges(project_id)?;
        let mut adj: HashMap<String, Vec<String>> = HashMap::new();
        for e in edges {
            if e.kind == EdgeKind::DependsOn {
                adj.entry(e.source_id).or_default().push(e.target_id);
            }
        }
        let mut set: HashSet<String> = HashSet::new();
        let mut stack = vec![root.to_owned()];
        while let Some(n) = stack.pop() {
            if set.insert(n.clone()) {
                if let Some(targets) = adj.get(&n) {
                    stack.extend(targets.iter().cloned());
                }
            }
        }
        Ok(set.into_iter().collect())
    }

    /// Recompute a node's content hash over `(statement, sorted dependency
    /// target ids, provenance)` — a fingerprint that changes when the node's
    /// dependencies change, not only its own text. Callers opt in; the initial
    /// hash set at creation time is left untouched.
    pub fn recompute_content_hash(&self, project_id: &str, node_id: &str) -> Result<String> {
        let nodes = self.nodes(project_id)?;
        let node = nodes
            .iter()
            .find(|n| n.id == node_id)
            .ok_or_else(|| anyhow!("node not found: {node_id}"))?;
        let mut deps: Vec<String> = self
            .edges(project_id)?
            .into_iter()
            .filter(|e| e.source_id == node_id && e.kind == EdgeKind::DependsOn)
            .map(|e| e.target_id)
            .collect();
        deps.sort();
        let material = format!("{}|{}|{}", node.statement, deps.join(","), node.provenance);
        let hash = hex_lower(Sha256::digest(material));
        self.conn.execute(
            "UPDATE nodes SET content_hash=?1,updated_at=?2 WHERE id=?3 AND project_id=?4",
            params![hash, Utc::now().to_rfc3339(), node_id, project_id],
        )?;
        Ok(hash)
    }

    pub fn recompute_all_hashes(&self, project_id: &str) -> Result<()> {
        for node in self.nodes(project_id)? {
            self.recompute_content_hash(project_id, &node.id)?;
        }
        Ok(())
    }

    /// Extract a reusable lemma from a verified subgraph (Alethfeld's context-
    /// compression op). The extraction set is the root plus its transitive
    /// DependsOn ancestors; every non-assumption node in it must be verified,
    /// assumptions become the lemma's hypotheses, and the lemma is tainted if
    /// any node in the set is tainted.
    pub fn extract_lemma(&self, project_id: &str, root_node_id: &str, name: &str) -> Result<Lemma> {
        let nodes = self.nodes(project_id)?;
        let by_id: HashMap<&str, &Node> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let root = *by_id
            .get(root_node_id)
            .ok_or_else(|| anyhow!("node not found: {root_node_id}"))?;
        let closure = self.dependency_closure(project_id, root_node_id)?;
        let mut assumptions: Vec<String> = Vec::new();
        let mut taint = false;
        for id in &closure {
            let Some(node) = by_id.get(id.as_str()).copied() else {
                continue;
            };
            if node.tainted {
                taint = true;
            }
            if node.kind == NodeKind::Assumption {
                assumptions.push(node.statement.clone());
                continue;
            }
            if !matches!(
                node.status,
                NodeStatus::InformallyVerified | NodeStatus::FormallyVerified
            ) {
                return Err(anyhow!(
                    "cannot extract lemma: node '{}' ({}) is not verified",
                    node.title,
                    node.status
                ));
            }
        }
        assumptions.sort();
        let statement = if assumptions.is_empty() {
            root.statement.clone()
        } else {
            format!("If {}, then {}", assumptions.join(" and "), root.statement)
        };
        let now = Utc::now();
        let lemma = Lemma {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            name: name.to_owned(),
            statement,
            source_node_id: root_node_id.to_owned(),
            taint,
            created_at: now,
        };
        self.conn.execute(
            "INSERT INTO lemmas VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                lemma.id,
                lemma.project_id,
                lemma.name,
                lemma.statement,
                lemma.source_node_id,
                taint as i64,
                now.to_rfc3339()
            ],
        )?;
        self.event(
            Some(project_id),
            None,
            "lemma.extracted",
            "extraction",
            json!({"lemma_id":lemma.id,"name":name,"taint":taint}),
        )?;
        Ok(lemma)
    }

    pub fn lemmas(&self, project_id: &str) -> Result<Vec<Lemma>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,name,statement,source_node_id,taint,created_at
             FROM lemmas WHERE project_id=?1 ORDER BY created_at",
        )?;
        let rows = st.query_map([project_id], lemma_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- Growing verified-lemma library (LEGO-Prover) ---------------------
    //
    // Persistence for the three logical stores behind
    // `reason::proving::library::LemmaLibrary`. All admission / dedup / ranking
    // *policy* lives in that module; these methods are pure CRUD over the
    // `library_lemmas` / `library_requests` / `library_problems` tables.

    /// Insert an admitted skill into the lemma store. `embedding_key` is a
    /// deterministic fingerprint of the statement supplied by the caller (the
    /// library computes it); `update_count` starts at 0.
    pub fn add_library_lemma(
        &self,
        project_id: &str,
        statement: &str,
        proof: &str,
        provenance: &str,
        embedding_key: &str,
    ) -> Result<LibraryLemma> {
        self.project(project_id)?;
        let now = Utc::now();
        let lemma = LibraryLemma {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            statement: statement.to_owned(),
            proof: proof.to_owned(),
            provenance: provenance.to_owned(),
            embedding_key: embedding_key.to_owned(),
            update_count: 0,
            created_at: now,
            updated_at: now,
        };
        self.conn.execute(
            "INSERT INTO library_lemmas(id,project_id,statement,proof,provenance,embedding_key,\
             update_count,created_at,updated_at) VALUES (?1,?2,?3,?4,?5,?6,0,?7,?7)",
            params![
                lemma.id,
                project_id,
                statement,
                proof,
                provenance,
                embedding_key,
                now.to_rfc3339()
            ],
        )?;
        self.event(
            Some(project_id),
            None,
            "library.lemma_admitted",
            "library",
            json!({"lemma_id": lemma.id}),
        )?;
        Ok(lemma)
    }

    /// All admitted library lemmas for a project, in insertion order.
    pub fn library_lemmas(&self, project_id: &str) -> Result<Vec<LibraryLemma>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,statement,proof,provenance,embedding_key,update_count,\
             created_at,updated_at FROM library_lemmas WHERE project_id=?1 \
             ORDER BY created_at, id",
        )?;
        let rows = st.query_map([project_id], library_lemma_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The least-`update_count` lemma (the evolver scheduler pick). Ties broken
    /// by oldest-created then id, so the choice is deterministic.
    pub fn next_library_lemma_to_evolve(&self, project_id: &str) -> Result<Option<LibraryLemma>> {
        self.conn
            .query_row(
                "SELECT id,project_id,statement,proof,provenance,embedding_key,update_count,\
                 created_at,updated_at FROM library_lemmas WHERE project_id=?1 \
                 ORDER BY update_count ASC, created_at ASC, id ASC LIMIT 1",
                [project_id],
                library_lemma_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Increment a lemma's `update_count` (it has just been worked on).
    pub fn bump_library_lemma_update(&self, project_id: &str, id: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE library_lemmas SET update_count=update_count+1,updated_at=?1 \
             WHERE id=?2 AND project_id=?3",
            params![Utc::now().to_rfc3339(), id, project_id],
        )?;
        if changed == 0 {
            return Err(anyhow!("library lemma not found: {id}"));
        }
        Ok(())
    }

    /// Enqueue an open sub-goal (from a sketch hole) into the request store.
    pub fn add_library_request(
        &self,
        project_id: &str,
        subgoal: &str,
        provenance: &str,
    ) -> Result<LibraryRequest> {
        self.project(project_id)?;
        let now = Utc::now();
        let req = LibraryRequest {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            subgoal: subgoal.to_owned(),
            provenance: provenance.to_owned(),
            solved: false,
            update_count: 0,
            created_at: now,
            updated_at: now,
        };
        self.conn.execute(
            "INSERT INTO library_requests(id,project_id,subgoal,provenance,solved,update_count,\
             created_at,updated_at) VALUES (?1,?2,?3,?4,0,0,?5,?5)",
            params![req.id, project_id, subgoal, provenance, now.to_rfc3339()],
        )?;
        self.event(
            Some(project_id),
            None,
            "library.request_enqueued",
            "library",
            json!({"request_id": req.id}),
        )?;
        Ok(req)
    }

    /// All requests for a project, in insertion order.
    pub fn library_requests(&self, project_id: &str) -> Result<Vec<LibraryRequest>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,subgoal,provenance,solved,update_count,created_at,updated_at \
             FROM library_requests WHERE project_id=?1 ORDER BY created_at, id",
        )?;
        let rows = st.query_map([project_id], library_request_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The oldest still-open request (least `update_count`, then oldest-created).
    pub fn oldest_open_library_request(&self, project_id: &str) -> Result<Option<LibraryRequest>> {
        self.conn
            .query_row(
                "SELECT id,project_id,subgoal,provenance,solved,update_count,created_at,updated_at \
                 FROM library_requests WHERE project_id=?1 AND solved=0 \
                 ORDER BY update_count ASC, created_at ASC, id ASC LIMIT 1",
                [project_id],
                library_request_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Mark a request solved (a lemma discharging it was admitted).
    pub fn mark_library_request_solved(&self, project_id: &str, id: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE library_requests SET solved=1,updated_at=?1 WHERE id=?2 AND project_id=?3",
            params![Utc::now().to_rfc3339(), id, project_id],
        )?;
        if changed == 0 {
            return Err(anyhow!("library request not found: {id}"));
        }
        Ok(())
    }

    /// Increment a request's `update_count` (it has just been worked on).
    pub fn bump_library_request_update(&self, project_id: &str, id: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE library_requests SET update_count=update_count+1,updated_at=?1 \
             WHERE id=?2 AND project_id=?3",
            params![Utc::now().to_rfc3339(), id, project_id],
        )?;
        if changed == 0 {
            return Err(anyhow!("library request not found: {id}"));
        }
        Ok(())
    }

    /// Record a target statement that biases evolution (the problem store).
    pub fn add_library_problem(
        &self,
        project_id: &str,
        statement: &str,
        provenance: &str,
    ) -> Result<LibraryProblem> {
        self.project(project_id)?;
        let now = Utc::now();
        let problem = LibraryProblem {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            statement: statement.to_owned(),
            provenance: provenance.to_owned(),
            created_at: now,
        };
        self.conn.execute(
            "INSERT INTO library_problems(id,project_id,statement,provenance,created_at) \
             VALUES (?1,?2,?3,?4,?5)",
            params![
                problem.id,
                project_id,
                statement,
                provenance,
                now.to_rfc3339()
            ],
        )?;
        Ok(problem)
    }

    /// All target problems for a project, in insertion order.
    pub fn library_problems(&self, project_id: &str) -> Result<Vec<LibraryProblem>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,statement,provenance,created_at FROM library_problems \
             WHERE project_id=?1 ORDER BY created_at, id",
        )?;
        let rows = st.query_map([project_id], library_problem_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // --- Global persistent goal cache (AlphaProof "Nexus") -----------------
    //
    // Persistence for `reason::search::goal_cache::GoalCache`. All
    // canonicalization / subsumption *policy* lives in that module; these
    // methods are pure CRUD over the `goal_cache` table.

    /// Insert a cached proof keyed by its caller-supplied `canonical_key` (the
    /// canonical form of `goal`). Does not deduplicate — the `GoalCache` policy
    /// checks [`Store::goal_cache_by_key`] first to stay idempotent on a key.
    pub fn add_goal_cache_entry(
        &self,
        project_id: &str,
        canonical_key: &str,
        goal: &str,
        proof: &str,
    ) -> Result<GoalCacheEntry> {
        self.project(project_id)?;
        let now = Utc::now();
        let entry = GoalCacheEntry {
            id: Uuid::new_v4().to_string(),
            project_id: project_id.to_owned(),
            canonical_key: canonical_key.to_owned(),
            goal: goal.to_owned(),
            proof: proof.to_owned(),
            created_at: now,
        };
        self.conn.execute(
            "INSERT INTO goal_cache(id,project_id,canonical_key,goal,proof,created_at) \
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                entry.id,
                project_id,
                canonical_key,
                goal,
                proof,
                now.to_rfc3339()
            ],
        )?;
        self.event(
            Some(project_id),
            None,
            "goal_cache.stored",
            "goal_cache",
            json!({"entry_id": entry.id}),
        )?;
        Ok(entry)
    }

    /// The cached entry for a canonical key, if any (the exact-hit lookup).
    pub fn goal_cache_by_key(
        &self,
        project_id: &str,
        canonical_key: &str,
    ) -> Result<Option<GoalCacheEntry>> {
        self.conn
            .query_row(
                "SELECT id,project_id,canonical_key,goal,proof,created_at FROM goal_cache \
                 WHERE project_id=?1 AND canonical_key=?2 ORDER BY created_at, id LIMIT 1",
                params![project_id, canonical_key],
                goal_cache_entry_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// All cached entries for a project, in insertion order (the subsumption
    /// scan iterates these).
    pub fn goal_cache_entries(&self, project_id: &str) -> Result<Vec<GoalCacheEntry>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,canonical_key,goal,proof,created_at FROM goal_cache \
             WHERE project_id=?1 ORDER BY created_at, id",
        )?;
        let rows = st.query_map([project_id], goal_cache_entry_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Number of cached entries for a project.
    pub fn count_goal_cache(&self, project_id: &str) -> Result<usize> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM goal_cache WHERE project_id=?1",
            [project_id],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }
}

fn dt(s: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&s)
        .map(|x| x.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                s.len(),
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })
}
fn project_row(r: &Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        name: r.get(1)?,
        theorem: r.get(2)?,
        created_at: dt(r.get(3)?)?,
        updated_at: dt(r.get(4)?)?,
    })
}
fn node_row(r: &Row) -> rusqlite::Result<Node> {
    let kind: String = r.get(2)?;
    let status: String = r.get(3)?;
    let tier: String = r.get(10)?;
    let lemmas_raw: String = r.get(13)?;
    let lean_decls_raw: String = r.get(14)?;
    let tainted_bool: i64 = r.get(9)?;
    let taint_str: String = r.get(19)?;
    // Reconcile the two columns for backward compatibility: a row written before
    // the `taint` column existed has `taint='clean'` (the default) but may carry
    // the old `tainted=1` bit — treat that as `Tainted`. Otherwise the explicit
    // three-valued column wins.
    let taint = if taint_str == "clean" && tainted_bool != 0 {
        Taint::Tainted
    } else {
        taint_str.parse().map_err(sql_parse)?
    };
    Ok(Node {
        id: r.get(0)?,
        project_id: r.get(1)?,
        kind: kind.parse().map_err(sql_parse)?,
        status: status.parse().map_err(sql_parse)?,
        title: r.get(4)?,
        statement: r.get(5)?,
        formal_statement: r.get(6)?,
        provenance: r.get(7)?,
        content_hash: r.get(8)?,
        tainted: taint.is_tainted(),
        taint,
        tier: tier.parse().map_err(sql_parse)?,
        parent_id: r.get(11)?,
        strategy_hint: r.get(12)?,
        suggested_lemmas: serde_json::from_str(&lemmas_raw).unwrap_or_default(),
        lean_decls: serde_json::from_str(&lean_decls_raw).unwrap_or_default(),
        created_at: dt(r.get(15)?)?,
        updated_at: dt(r.get(16)?)?,
        stmt_formalized: r.get::<_, i64>(17)? != 0,
        proof_done: r.get::<_, i64>(18)? != 0,
    })
}
fn edge_row(r: &Row) -> rusqlite::Result<Edge> {
    let kind: String = r.get(4)?;
    let strength: String = r.get(5)?;
    let dep_scope: String = r.get(6)?;
    Ok(Edge {
        id: r.get(0)?,
        project_id: r.get(1)?,
        source_id: r.get(2)?,
        target_id: r.get(3)?,
        kind: kind.parse().map_err(sql_parse)?,
        evidence_strength: strength.parse().map_err(sql_parse)?,
        dep_scope: dep_scope.parse().map_err(sql_parse)?,
        created_at: dt(r.get(7)?)?,
    })
}
fn event_row(r: &Row) -> rusqlite::Result<Event> {
    let raw: String = r.get(5)?;
    Ok(Event {
        id: r.get(0)?,
        project_id: r.get(1)?,
        run_id: r.get(2)?,
        event_type: r.get(3)?,
        actor: r.get(4)?,
        payload: serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw)),
        created_at: dt(r.get(6)?)?,
    })
}
fn proposal_row(r: &Row) -> rusqlite::Result<Proposal> {
    let raw: String = r.get(2)?;
    let resolved: Option<String> = r.get(7)?;
    Ok(Proposal {
        id: r.get(0)?,
        project_id: r.get(1)?,
        action: serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw)),
        status: r.get(3)?,
        proposed_by: r.get(4)?,
        resolution_note: r.get(5)?,
        created_at: dt(r.get(6)?)?,
        resolved_at: resolved.map(dt).transpose()?,
    })
}
fn attempt_row(r: &Row) -> rusqlite::Result<Attempt> {
    let input_raw: String = r.get(5)?;
    let output_raw: String = r.get(6)?;
    Ok(Attempt {
        id: r.get(0)?,
        project_id: r.get(1)?,
        node_id: r.get(2)?,
        run_id: r.get(3)?,
        actor: r.get(4)?,
        input: serde_json::from_str(&input_raw).unwrap_or(serde_json::Value::String(input_raw)),
        output: serde_json::from_str(&output_raw).unwrap_or(serde_json::Value::String(output_raw)),
        success: r.get::<_, i64>(7)? != 0,
        created_at: dt(r.get(8)?)?,
    })
}
fn lemma_row(r: &Row) -> rusqlite::Result<Lemma> {
    Ok(Lemma {
        id: r.get(0)?,
        project_id: r.get(1)?,
        name: r.get(2)?,
        statement: r.get(3)?,
        source_node_id: r.get(4)?,
        taint: r.get::<_, i64>(5)? != 0,
        created_at: dt(r.get(6)?)?,
    })
}

/// An admitted skill in the growing verified-lemma library. Distinct from the
/// legacy graph-extraction [`crate::model::Lemma`]: this is the LEGO-Prover
/// "lemma store" record — a verified `(statement, proof)` with its provenance,
/// a deterministic `embedding_key`, and an `update_count` used by the evolver
/// scheduler.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryLemma {
    pub id: String,
    pub project_id: String,
    pub statement: String,
    pub proof: String,
    pub provenance: String,
    pub embedding_key: String,
    pub update_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A conjectured open sub-goal in the library "request store" — doubles as a
/// retrieval query and an item on the evolver worklist.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryRequest {
    pub id: String,
    pub project_id: String,
    pub subgoal: String,
    pub provenance: String,
    pub solved: bool,
    pub update_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A target statement in the library "problem store" that biases evolution.
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryProblem {
    pub id: String,
    pub project_id: String,
    pub statement: String,
    pub provenance: String,
    pub created_at: DateTime<Utc>,
}

fn library_lemma_row(r: &Row) -> rusqlite::Result<LibraryLemma> {
    Ok(LibraryLemma {
        id: r.get(0)?,
        project_id: r.get(1)?,
        statement: r.get(2)?,
        proof: r.get(3)?,
        provenance: r.get(4)?,
        embedding_key: r.get(5)?,
        update_count: r.get(6)?,
        created_at: dt(r.get(7)?)?,
        updated_at: dt(r.get(8)?)?,
    })
}

fn library_request_row(r: &Row) -> rusqlite::Result<LibraryRequest> {
    Ok(LibraryRequest {
        id: r.get(0)?,
        project_id: r.get(1)?,
        subgoal: r.get(2)?,
        provenance: r.get(3)?,
        solved: r.get::<_, i64>(4)? != 0,
        update_count: r.get(5)?,
        created_at: dt(r.get(6)?)?,
        updated_at: dt(r.get(7)?)?,
    })
}

fn library_problem_row(r: &Row) -> rusqlite::Result<LibraryProblem> {
    Ok(LibraryProblem {
        id: r.get(0)?,
        project_id: r.get(1)?,
        statement: r.get(2)?,
        provenance: r.get(3)?,
        created_at: dt(r.get(4)?)?,
    })
}

/// A cached proof in the global persistent goal cache (AlphaProof "Nexus"): a
/// verified `(goal, proof)` keyed by the goal's canonical form so that proven
/// sub-goals are reused across searches and runs.
#[derive(Debug, Clone, PartialEq)]
pub struct GoalCacheEntry {
    pub id: String,
    pub project_id: String,
    /// The canonical key of `goal` (supplied by `GoalCache`).
    pub canonical_key: String,
    /// The original goal text as stored (used for subsumption checks).
    pub goal: String,
    pub proof: String,
    pub created_at: DateTime<Utc>,
}

fn goal_cache_entry_row(r: &Row) -> rusqlite::Result<GoalCacheEntry> {
    Ok(GoalCacheEntry {
        id: r.get(0)?,
        project_id: r.get(1)?,
        canonical_key: r.get(2)?,
        goal: r.get(3)?,
        proof: r.get(4)?,
        created_at: dt(r.get(5)?)?,
    })
}

fn proof_job_row(r: &Row) -> rusqlite::Result<crate::prover::model::ProofJob> {
    let status: String = r.get(4)?;
    let task_raw: String = r.get(5)?;
    let result_raw: Option<String> = r.get(6)?;
    let artifacts: Option<String> = r.get(9)?;
    let completed: Option<String> = r.get(13)?;
    Ok(crate::prover::model::ProofJob {
        id: r.get(0)?,
        project_id: r.get(1)?,
        node_id: r.get(2)?,
        backend: r.get(3)?,
        status: serde_json::from_str(&status)
            .unwrap_or(crate::prover::model::ProverJobStatus::Error),
        task: serde_json::from_str(&task_raw).map_err(|e| sql_parse(e.into()))?,
        result: result_raw
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| sql_parse(e.into()))?,
        external_id: r.get(7)?,
        percent_complete: r.get(8)?,
        artifacts_dir: artifacts.map(std::path::PathBuf::from),
        poll_count: r.get::<_, i64>(10)? as u32,
        submitted_at: dt(r.get(11)?)?,
        updated_at: dt(r.get(12)?)?,
        completed_at: completed.map(dt).transpose()?,
    })
}

fn attempt_run_row(r: &Row) -> rusqlite::Result<crate::prover::model::AttemptRunRecord> {
    let status: String = r.get(4)?;
    let input_raw: String = r.get(6)?;
    let output_raw: Option<String> = r.get(7)?;
    Ok(crate::prover::model::AttemptRunRecord {
        id: r.get(0)?,
        project_id: r.get(1)?,
        node_id: r.get(2)?,
        proof_job_id: r.get(3)?,
        status: serde_json::from_str(&status)
            .unwrap_or(crate::prover::model::AttemptRunStatus::Failed),
        artifacts_dir: std::path::PathBuf::from(r.get::<_, String>(5)?),
        input: serde_json::from_str(&input_raw).map_err(|e| sql_parse(e.into()))?,
        output: output_raw
            .filter(|s| !s.is_empty())
            .map(|s| serde_json::from_str(&s))
            .transpose()
            .map_err(|e| sql_parse(e.into()))?,
        started_at: dt(r.get(8)?)?,
        updated_at: dt(r.get(9)?)?,
        completed_at: r.get::<_, Option<String>>(10)?.map(dt).transpose()?,
        duration_ms: r.get::<_, Option<i64>>(11)?.map(|v| v as u128),
        cost: r.get(12)?,
    })
}

fn sql_parse(e: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_cycles_and_propagates_taint() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let a = s
            .add_node(&p.id, NodeKind::Lemma, "a", "a", "test")
            .unwrap();
        let b = s
            .add_node(&p.id, NodeKind::Obligation, "b", "b", "test")
            .unwrap();
        s.add_edge(&p.id, &a.id, &b.id, EdgeKind::DependsOn)
            .unwrap();
        assert!(s
            .add_edge(&p.id, &b.id, &a.id, EdgeKind::DependsOn)
            .is_err());
        s.set_node_status(&p.id, &b.id, NodeStatus::Rejected, "test")
            .unwrap();
        let nodes = s.nodes(&p.id).unwrap();
        assert!(nodes.iter().find(|n| n.id == a.id).unwrap().tainted);
    }

    #[test]
    fn content_hash_covers_dependencies() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let a = s
            .add_node(&p.id, NodeKind::Lemma, "a", "a", "test")
            .unwrap();
        let b = s
            .add_node(&p.id, NodeKind::Lemma, "b", "b", "test")
            .unwrap();
        let h0 = s.recompute_content_hash(&p.id, &a.id).unwrap();
        s.add_edge(&p.id, &a.id, &b.id, EdgeKind::DependsOn)
            .unwrap();
        let h1 = s.recompute_content_hash(&p.id, &a.id).unwrap();
        assert_ne!(h0, h1, "hash must change when a dependency is added");
    }

    #[test]
    fn extracts_lemma_from_verified_subgraph() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let assume = s
            .add_node(&p.id, NodeKind::Assumption, "h", "n is even", "test")
            .unwrap();
        let root = s
            .add_node(&p.id, NodeKind::Lemma, "root", "n squared is even", "test")
            .unwrap();
        s.add_edge(&p.id, &root.id, &assume.id, EdgeKind::DependsOn)
            .unwrap();
        // Unverified root: extraction must refuse.
        assert!(s.extract_lemma(&p.id, &root.id, "even_square").is_err());
        s.set_node_status(&p.id, &root.id, NodeStatus::FormallyVerified, "test")
            .unwrap();
        let lemma = s.extract_lemma(&p.id, &root.id, "even_square").unwrap();
        assert_eq!(lemma.statement, "If n is even, then n squared is even");
        let all = s.lemmas(&p.id).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "even_square");
    }

    #[test]
    fn taint_propagates_over_derived_from() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let base = s
            .add_node(&p.id, NodeKind::Lemma, "base", "b", "test")
            .unwrap();
        let derived = s
            .add_node(&p.id, NodeKind::Lemma, "derived", "d", "test")
            .unwrap();
        s.add_edge(&p.id, &derived.id, &base.id, EdgeKind::DerivedFrom)
            .unwrap();
        s.set_edge_strength(
            &p.id,
            &derived.id,
            &base.id,
            EdgeKind::DerivedFrom,
            EdgeStrength::LeanChecked,
        )
        .unwrap();
        s.set_node_status(&p.id, &base.id, NodeStatus::Blocked, "test")
            .unwrap();
        let nodes = s.nodes(&p.id).unwrap();
        assert!(nodes.iter().find(|n| n.id == derived.id).unwrap().tainted);
        let edge = s.edges(&p.id).unwrap().into_iter().next().unwrap();
        assert_eq!(edge.evidence_strength, EdgeStrength::LeanChecked);
    }

    #[test]
    fn statement_and_proof_dep_scopes_split_and_merge() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let root = s
            .add_node(&p.id, NodeKind::Lemma, "root", "R", "test")
            .unwrap();
        let a = s
            .add_node(&p.id, NodeKind::Lemma, "a", "A", "test")
            .unwrap();
        let b = s
            .add_node(&p.id, NodeKind::Lemma, "b", "B", "test")
            .unwrap();
        // A is used only to prove root; B is used in the statement itself.
        s.add_edge_scoped(&p.id, &root.id, &a.id, EdgeKind::DependsOn, DepScope::Proof)
            .unwrap();
        s.add_edge_scoped(
            &p.id,
            &root.id,
            &b.id,
            EdgeKind::DependsOn,
            DepScope::Statement,
        )
        .unwrap();
        // The statement also ends up needing A: widen to Both.
        s.widen_edge_scope(
            &p.id,
            &root.id,
            &a.id,
            EdgeKind::DependsOn,
            DepScope::Statement,
        )
        .unwrap();
        let edges = s.edges(&p.id).unwrap();
        let ea = edges.iter().find(|e| e.target_id == a.id).unwrap();
        let eb = edges.iter().find(|e| e.target_id == b.id).unwrap();
        assert_eq!(ea.dep_scope, DepScope::Both);
        assert_eq!(eb.dep_scope, DepScope::Statement);
        // Legacy plain add_edge defaults to statement scope.
        let c = s
            .add_node(&p.id, NodeKind::Lemma, "c", "C", "test")
            .unwrap();
        s.add_edge(&p.id, &root.id, &c.id, EdgeKind::DependsOn)
            .unwrap();
        let ec = s
            .edges(&p.id)
            .unwrap()
            .into_iter()
            .find(|e| e.target_id == c.id)
            .unwrap();
        assert_eq!(ec.dep_scope, DepScope::Statement);
    }

    #[test]
    fn verification_flags_track_statement_and_proof_independently() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let n = s
            .add_node(&p.id, NodeKind::Lemma, "n", "N", "test")
            .unwrap();
        // Fresh node: neither formalized nor proved.
        let fresh = s.nodes(&p.id).unwrap().into_iter().next().unwrap();
        assert!(!fresh.stmt_formalized && !fresh.proof_done);
        // Statement formalized, proof still open — the blueprint mid-state.
        s.set_verification_flags(&p.id, &n.id, true, false, "test")
            .unwrap();
        let mid = s.nodes(&p.id).unwrap().into_iter().next().unwrap();
        assert!(mid.stmt_formalized && !mid.proof_done);
    }

    #[test]
    fn records_attempts() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let n = s
            .add_node(&p.id, NodeKind::Obligation, "o", "s", "test")
            .unwrap();
        s.add_attempt(
            &p.id,
            Some(&n.id),
            None,
            "lean",
            &json!({"file":"m.lean"}),
            &json!({"ok":false}),
            false,
        )
        .unwrap();
        s.add_attempt(
            &p.id,
            Some(&n.id),
            None,
            "python_check",
            &json!({"tool":"falsify"}),
            &json!({"verdict":"no_counterexample"}),
            true,
        )
        .unwrap();
        let got = s.attempts(&p.id, 10).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got.iter().filter(|a| a.success).count(), 1);
        assert_eq!(
            got.iter()
                .filter(|a| a.node_id.as_deref() == Some(n.id.as_str()))
                .count(),
            2
        );
    }

    #[test]
    fn stores_node_blueprint_metadata() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let spine = s
            .add_node(&p.id, NodeKind::Obligation, "spine", "S", "test")
            .unwrap();
        let child = s
            .add_node_detailed(
                &p.id,
                NodeKind::Lemma,
                NodeTier::Implementation,
                Some(&spine.id),
                "impl",
                "L",
                Some("induct on n"),
                &["Nat.succ_le_succ".to_string()],
                "agent",
            )
            .unwrap();
        s.set_suggested_lemmas(&p.id, &spine.id, &["Nat.mul_comm".to_string()], "agent")
            .unwrap();
        s.set_strategy_hint(&p.id, &spine.id, Some("contrapositive"), "agent")
            .unwrap();
        let nodes = s.nodes(&p.id).unwrap();
        let got_spine = nodes.iter().find(|n| n.id == spine.id).unwrap();
        let got_child = nodes.iter().find(|n| n.id == child.id).unwrap();
        assert_eq!(got_spine.tier, NodeTier::Spine);
        assert_eq!(got_spine.strategy_hint.as_deref(), Some("contrapositive"));
        assert_eq!(got_spine.suggested_lemmas, vec!["Nat.mul_comm".to_string()]);
        assert!(got_spine.lean_decls.is_empty());
        assert_eq!(got_child.tier, NodeTier::Implementation);
        assert_eq!(got_child.parent_id.as_deref(), Some(spine.id.as_str()));
        assert_eq!(got_child.strategy_hint.as_deref(), Some("induct on n"));
        assert_eq!(
            got_child.suggested_lemmas,
            vec!["Nat.succ_le_succ".to_string()]
        );
    }

    #[test]
    fn stores_lean_declaration_bindings() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let n = s
            .add_node(&p.id, NodeKind::Lemma, "n", "N", "test")
            .unwrap();
        s.set_lean_decls(
            &p.id,
            &n.id,
            &["Ns.main".to_string(), "Ns.helper".to_string()],
            "blueprint",
        )
        .unwrap();
        let got = s.nodes(&p.id).unwrap().into_iter().next().unwrap();
        assert_eq!(got.lean_decls, vec!["Ns.main", "Ns.helper"]);
    }

    #[test]
    fn cycle_rejection_leaves_no_partial_edge() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let a = s
            .add_node(&p.id, NodeKind::Lemma, "a", "a", "test")
            .unwrap();
        let b = s
            .add_node(&p.id, NodeKind::Lemma, "b", "b", "test")
            .unwrap();
        s.add_edge(&p.id, &a.id, &b.id, EdgeKind::DependsOn)
            .unwrap();
        let edges_before = s.edges(&p.id).unwrap();
        // b -> a would close a cycle: it must be rejected AND leave the edge set
        // exactly as it was — no lingering half-inserted row from the aborted
        // insert-then-check.
        let err = s
            .add_edge(&p.id, &b.id, &a.id, EdgeKind::DependsOn)
            .unwrap_err();
        assert!(err.to_string().contains("cycle"));
        let edges_after = s.edges(&p.id).unwrap();
        assert_eq!(edges_after.len(), edges_before.len());
        assert!(edges_after
            .iter()
            .all(|e| !(e.source_id == b.id && e.target_id == a.id)));
    }

    #[test]
    fn status_change_emits_event_atomically() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let n = s
            .add_node(&p.id, NodeKind::Lemma, "n", "N", "test")
            .unwrap();
        s.set_node_status(&p.id, &n.id, NodeStatus::InformallyVerified, "test")
            .unwrap();
        let got = s
            .nodes(&p.id)
            .unwrap()
            .into_iter()
            .find(|x| x.id == n.id)
            .unwrap();
        assert_eq!(got.status, NodeStatus::InformallyVerified);
        let evented = s
            .events(&p.id, 10_000)
            .unwrap()
            .iter()
            .any(|e| e.event_type == "node.status_changed");
        assert!(evented, "status change must emit its event");
        // A status change against a missing node fails and writes nothing.
        assert!(s
            .set_node_status(&p.id, "does-not-exist", NodeStatus::Rejected, "test")
            .is_err());
    }

    #[test]
    fn merge_rolls_back_when_an_edge_fails() {
        let s = Store::open(Path::new(":memory:")).unwrap();
        let p = s.create_project("p", "t").unwrap();
        let parent = s
            .add_node(&p.id, NodeKind::Lemma, "parent", "P", "test")
            .unwrap();
        let nodes_before = s.nodes(&p.id).unwrap().len();
        let edges_before = s.edges(&p.id).unwrap().len();
        // The second "parent" id does not exist, so its edge INSERT hits a
        // foreign-key violation partway through the merge. The whole merge —
        // the new node, its first (valid) edge, and the events — must roll back.
        let res = s.merge_nodes(
            &p.id,
            NodeKind::Lemma,
            "merged",
            "M",
            &[parent.id.clone(), "does-not-exist".to_string()],
            "test",
        );
        assert!(res.is_err());
        assert_eq!(s.nodes(&p.id).unwrap().len(), nodes_before);
        assert_eq!(s.edges(&p.id).unwrap().len(), edges_before);
        assert!(s
            .events(&p.id, 10_000)
            .unwrap()
            .iter()
            .all(|e| e.event_type != "nodes.merged"));
    }
}
