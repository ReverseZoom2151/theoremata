use crate::model::*;
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::json;
use sha2::{Digest, Sha256};
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
              created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS edges (
              id INTEGER PRIMARY KEY AUTOINCREMENT,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              source_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
              target_id TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
              kind TEXT NOT NULL, evidence_strength TEXT NOT NULL DEFAULT 'numeric_screen',
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
            CREATE INDEX IF NOT EXISTS idx_nodes_project ON nodes(project_id);
            CREATE INDEX IF NOT EXISTS idx_edges_project ON edges(project_id);
            CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, id);
            CREATE INDEX IF NOT EXISTS idx_messages_project ON messages(project_id, id);
            "#,
        )?;
        // Idempotent column additions so databases created before these fields
        // existed are upgraded in place (a fresh table already has them).
        for (column, decl) in [
            ("tier", "TEXT NOT NULL DEFAULT 'spine'"),
            ("parent_id", "TEXT"),
            ("strategy_hint", "TEXT"),
            ("suggested_lemmas", "TEXT NOT NULL DEFAULT '[]'"),
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
        for (column, decl) in [("evidence_strength", "TEXT NOT NULL DEFAULT 'numeric_screen'")] {
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
        let hash = format!(
            "{:x}",
            Sha256::digest(format!("{kind}|{title}|{statement}"))
        );
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
            tier,
            parent_id: parent_id.map(str::to_owned),
            strategy_hint: strategy_hint.map(str::to_owned),
            suggested_lemmas: suggested_lemmas.to_vec(),
            created_at: now,
            updated_at: now,
        };
        self.conn.execute(
            "INSERT INTO nodes(id,project_id,kind,status,title,statement,formal_statement,\
             provenance,content_hash,tainted,tier,parent_id,strategy_hint,suggested_lemmas,\
             created_at,updated_at) \
             VALUES (?1,?2,?3,?4,?5,?6,NULL,?7,?8,0,?9,?10,?11,?12,?13,?13)",
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

    pub fn set_node_status(
        &self,
        project_id: &str,
        node_id: &str,
        status: NodeStatus,
        actor: &str,
    ) -> Result<()> {
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

    pub fn add_edge(
        &self,
        project_id: &str,
        source: &str,
        target: &str,
        kind: EdgeKind,
    ) -> Result<()> {
        if source == target {
            return Err(anyhow!("self edges are not allowed"));
        }
        self.conn.execute(
            "INSERT INTO edges(project_id,source_id,target_id,kind,evidence_strength,created_at) \
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                project_id,
                source,
                target,
                kind.to_string(),
                EdgeStrength::NumericScreen.to_string(),
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
            serde_json::json!({"source":source,"target":target,"kind":kind}),
        )?;
        Ok(())
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
             content_hash,tainted,tier,parent_id,strategy_hint,suggested_lemmas,created_at,updated_at
             FROM nodes WHERE project_id=?1 ORDER BY created_at",
        )?;
        let rows = st.query_map([project_id], node_row)?;
        let values = rows.collect::<rusqlite::Result<_>>()?;
        Ok(values)
    }

    pub fn edges(&self, project_id: &str) -> Result<Vec<Edge>> {
        let mut st = self.conn.prepare(
            "SELECT id,project_id,source_id,target_id,kind,evidence_strength,created_at FROM edges WHERE project_id=?1 ORDER BY id",
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

    fn recompute_taint(&self, project_id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE nodes SET tainted=0 WHERE project_id=?1",
            [project_id],
        )?;
        let nodes = self.nodes(project_id)?;
        let edges = self.edges(project_id)?;
        let mut tainted: HashSet<String> = nodes
            .iter()
            .filter(|n| matches!(n.status, NodeStatus::Rejected | NodeStatus::Blocked))
            .map(|n| n.id.clone())
            .collect();
        loop {
            let before = tainted.len();
            for e in &edges {
                // Support edges carry taint from a rejected/blocked target up to
                // the node that relies on it; adversarial/replacement links
                // (Contradicts, Supersedes, Verifies) deliberately do not.
                if matches!(
                    e.kind,
                    EdgeKind::DependsOn | EdgeKind::DerivedFrom | EdgeKind::Formalizes
                ) && tainted.contains(&e.target_id)
                {
                    tainted.insert(e.source_id.clone());
                }
            }
            if tainted.len() == before {
                break;
            }
        }
        for id in tainted {
            self.conn
                .execute("UPDATE nodes SET tainted=1 WHERE id=?1", [id])?;
        }
        Ok(())
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
        tainted: r.get::<_, i64>(9)? != 0,
        tier: tier.parse().map_err(sql_parse)?,
        parent_id: r.get(11)?,
        strategy_hint: r.get(12)?,
        suggested_lemmas: serde_json::from_str(&lemmas_raw).unwrap_or_default(),
        created_at: dt(r.get(14)?)?,
        updated_at: dt(r.get(15)?)?,
    })
}
fn edge_row(r: &Row) -> rusqlite::Result<Edge> {
    let kind: String = r.get(4)?;
    let strength: String = r.get(5)?;
    Ok(Edge {
        id: r.get(0)?,
        project_id: r.get(1)?,
        source_id: r.get(2)?,
        target_id: r.get(3)?,
        kind: kind.parse().map_err(sql_parse)?,
        evidence_strength: strength.parse().map_err(sql_parse)?,
        created_at: dt(r.get(6)?)?,
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
        assert_eq!(got.iter().filter(|a| a.node_id.as_deref() == Some(n.id.as_str())).count(), 2);
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
        assert_eq!(got_child.tier, NodeTier::Implementation);
        assert_eq!(got_child.parent_id.as_deref(), Some(spine.id.as_str()));
        assert_eq!(got_child.strategy_hint.as_deref(), Some("induct on n"));
        assert_eq!(
            got_child.suggested_lemmas,
            vec!["Nat.succ_le_succ".to_string()]
        );
    }
}
