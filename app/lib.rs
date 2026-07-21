mod api;
mod config;
#[path = "../components/graph/mod.rs"]
mod graph;
#[path = "../components/prover/mod.rs"]
mod prover;
#[path = "../components/provider/mod.rs"]
mod provider;
#[path = "../components/reason/mod.rs"]
mod reason;
#[path = "../components/tools/mod.rs"]
mod tools;
mod tui;
#[path = "../components/verify/mod.rs"]
mod verify;

// Re-export the grouped modules at the crate root so every component source
// keeps its flat `crate::model` / `crate::workflow` / `crate::lean_session`
// paths unchanged — the physical layout is by component, the namespace is flat.
pub use graph::{citation, db, model, scheduler};
pub use prover::{
    aristotle, attempt_run, axiom_audit, decl_index_adapter, declaration_lookup, error_feedback,
    exec, formal, goal_state, hypothesis_audit, isabelle, lean, proof_job, proof_log, rocq,
    statement_preservation, subgoal_extract, vacuity,
};
pub use reason::{
    agent, alignment, alignment_propose, alignment_refute, best_first, blueprint,
    blueprint_generate, blueprint_run, certification, chat, checker_cache, concurrent,
    conjecture_engine, consolidate, context_assembly, critic, critic_scorer, dag_projection,
    decompose, decomposition_admission, definition_synthesis, discovery_game, distance_critic,
    driver, evolve_sketch, falsification, fitness, formal_generate, formalize_modes,
    formalize_portfolio, goal_cache, graph_rag, guard, guardrails, hybrid_search,
    informal_defect_prior, inverse_method, library, live_plan, mathlib_export, mcts, memory,
    meta_tools, method_transfer, minimize, model_elimination, model_router, observe, optimize,
    plan_history, portfolio, preference_pairs, process_reward, progress, proof_import, proof_pool,
    refine_ops, repair, research, retry, rewriting, router, sampler, sampling, search_telemetry,
    skest, sketch, statement_validation, statement_validity, subsumption, symmetry_dedup,
    tactic_outcome, taint, team, trace, ttc, validity_seams, verification_ladder,
};
pub use verify::{hardening, lean_session};

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use db::Store;
use model::{EdgeKind, NodeKind, NodeStatus};
use provider::{CommandProvider, ModelProvider, OfflineProvider};
use std::{fs, path::PathBuf};
use tools::{capability_report, LeanCheck, LeanParanoia, MathlibSearch, PythonCheck, Tool};

#[derive(Parser)]
#[command(
    name = "theoremata",
    version,
    about = "Graph-first mathematical research agent"
)]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    Init,
    Doctor,
    Mcp,
    New {
        name: String,
        theorem: String,
    },
    Projects,
    Status {
        project: String,
    },
    Graph {
        project: String,
    },
    AddNode {
        project: String,
        #[arg(value_enum)]
        kind: NodeKind,
        title: String,
        statement: String,
    },
    Link {
        project: String,
        source: String,
        target: String,
        #[arg(default_value = "depends_on")]
        kind: String,
    },
    SetStatus {
        project: String,
        node: String,
        #[arg(value_enum)]
        status: NodeStatus,
    },
    Chat {
        project: String,
    },
    Send {
        project: String,
        message: String,
    },
    Proposals {
        project: String,
        #[arg(long)]
        all: bool,
    },
    Approve {
        project: String,
        proposal: String,
    },
    Reject {
        project: String,
        proposal: String,
        #[arg(default_value = "rejected by user")]
        note: String,
    },
    Events {
        project: String,
        #[arg(default_value_t = 30)]
        limit: usize,
    },
    Attempts {
        project: String,
        #[arg(default_value_t = 30)]
        limit: usize,
    },
    Export {
        project: String,
        output: PathBuf,
    },
    Search {
        query: String,
        #[arg(default_value_t = 20)]
        limit: u64,
    },
    Compute {
        expression: String,
    },
    Falsify {
        variables: String,
        claim: String,
        #[arg(long, default_value = "True")]
        assumptions: String,
        #[arg(long, default_value_t = 100_000)]
        max_cases: u64,
    },
    Symbolic {
        #[arg(value_parser=["simplify","factor","expand","solve","differentiate","integrate"])]
        operation: String,
        expression: String,
        #[arg(long)]
        variable: Option<String>,
    },
    Estimates,
    Feasibility {
        constraints: String,
    },
    Asymptotic {
        request: String,
    },
    Grade {
        request: String,
    },
    Stages {
        request: String,
    },
    Imports {
        #[arg(default_value = "stats")]
        query: String,
        #[arg(long)]
        module: Option<String>,
        #[arg(long)]
        substring: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u64,
    },
    Decls {
        #[arg(default_value = "stats")]
        query: String,
        #[arg(long)]
        substring: Option<String>,
        #[arg(long)]
        kind: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u64,
        #[arg(long)]
        mathlib: bool,
    },
    Heads {
        #[arg(default_value = "stats")]
        query: String,
        #[arg(long)]
        head: Option<String>,
        #[arg(long)]
        pattern: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u64,
        #[arg(long)]
        mathlib: bool,
    },
    Soundness {
        file: PathBuf,
    },
    Axioms {
        file: PathBuf,
        theorem: String,
    },
    Schedule {
        project: String,
    },
    Paranoia {
        theorem: String,
    },
    Workspace {
        request: String,
    },
    LeanWarm {
        request: String,
    },
    /// Call any Python worker tool with a raw JSON request (e.g.
    /// `{"tool":"benchmark","request":{"op":"list"}}`).
    Tool {
        request: String,
    },
    /// Stable, versioned JSON API for editor/MCP clients (e.g.
    /// `{"op":"list_projects"}`). See `api::ApiRequest` for the schema.
    Api {
        request: String,
        /// Exact database to open for a process bridge. Kept hidden because
        /// normal CLI callers should use the configured database; `mcp` uses
        /// this to pass its already-resolved Store context to Python safely.
        #[arg(long, hide = true)]
        database: Option<PathBuf>,
    },
    /// Export a project's proof-DAG to a leanblueprint `content.tex` + `lean_decls`.
    BlueprintExport {
        project: String,
        out_dir: PathBuf,
    },
    /// Import a leanblueprint `content.tex` into a project's proof-DAG.
    BlueprintImport {
        project: String,
        file: PathBuf,
    },
    /// Drive a whole leanblueprint `content.tex` end-to-end: topo-order its
    /// `\uses` DAG and prove each item via the sketch + certification path,
    /// proving dependencies before dependents.
    BlueprintRun {
        project: String,
        file: PathBuf,
        /// Restrict each hole's portfolio to a comma-separated subset
        /// (e.g. `--systems lean,rocq`); defaults to all three.
        #[arg(long)]
        systems: Option<String>,
    },
    /// Referential-integrity gate: check every blueprint `\lean{}` decl resolves.
    Checkdecls {
        workspace: PathBuf,
        #[arg(long)]
        manifest: Option<PathBuf>,
    },
    Agent {
        project: String,
    },
    Harden {
        project: String,
        node: String,
    },
    Research {
        project: String,
    },
    Critique {
        project: String,
    },
    Route {
        project: String,
    },
    Consolidate {
        project: String,
    },
    Team {
        project: String,
    },
    Trace {
        project: String,
        #[arg(default_value_t = 30)]
        limit: usize,
    },
    Metrics {
        project: String,
    },
    Replay {
        project: String,
        #[arg(long)]
        run: Option<String>,
    },
    /// Phase 1.4 staleness census: walk stored verified results and classify each
    /// as Fresh / RepairCandidate / MathematicsMoved / Unknown under the current
    /// environment. Honest census only: it does not re-verify or repair.
    Sweep {
        /// Restrict to one project; omit to sweep every project.
        #[arg(long)]
        project: Option<String>,
        /// Max events read per project (newest first).
        #[arg(long, default_value_t = 100_000)]
        limit: usize,
        /// Import header to re-elaborate against when the stored record carries
        /// none. Defaults to `import Mathlib`.
        #[arg(long)]
        reelaborate_preamble: Option<String>,
        /// RAISE the per-sweep cap on how many stale pinned nodes may be
        /// re-elaborated. The discriminator always runs; this only buys more of
        /// it. Nodes are charged oldest-recorded first, since the oldest greens
        /// have had the longest to rot.
        #[arg(long, default_value_t = reason::proving::staleness_sweep::DEFAULT_MAX_REELABORATIONS)]
        max_reelaborate: usize,
    },
    Tactics {
        goal: String,
    },
    Strategies {
        goal: String,
    },
    SftExport {
        project: String,
    },
    Evolve {
        request: String,
    },
    Grpo {
        request: String,
    },
    Lemma {
        project: String,
        node: String,
        name: String,
    },
    Lemmas {
        project: String,
    },
    Rehash {
        project: String,
    },
    Eval {
        request: String,
    },
    Retrieve {
        query: String,
        #[arg(long)]
        mathlib: bool,
        #[arg(long, default_value_t = 20)]
        limit: u64,
    },
    Lean {
        file: PathBuf,
    },
    /// Generate AND verify a proof for a formal system (lean/rocq/isabelle):
    /// model-driven best-of-N selected by the live 3+1-layer gate.
    FormalProve {
        /// `lean` | `rocq` (`coq`) | `isabelle`.
        system: String,
        statement: String,
    },
    /// Force the HAMMER-assisted path: ask the `hammer` worker (Sledgehammer /
    /// CoqHammer / aesop) to FIND a tactic for the goal, assemble a complete
    /// system-native proof around it, and verify it through the live 3+1-layer
    /// gate. Prints the assembled proof and its VerificationReport.
    HammerProve {
        /// `lean` | `rocq` (`coq`) | `isabelle`.
        system: String,
        /// The goal, in the target system's native syntax
        /// (e.g. `1 + 1 = (2::nat)` for Isabelle).
        goal: String,
    },
    /// Attempt a conjecture across Lean, Rocq, and Isabelle and take whichever
    /// backend certifies first (the portfolio winner).
    PortfolioProve {
        statement: String,
        /// Restrict the portfolio to a comma-separated subset
        /// (e.g. `--systems lean,rocq`); defaults to all three.
        #[arg(long)]
        systems: Option<String>,
    },
    /// Submit an external proof job (ProofTask → async prover backend).
    ProofSubmit {
        project: String,
        statement: String,
        #[arg(long)]
        node: Option<String>,
        #[arg(long, default_value = "Theoremata.Main")]
        theorem: String,
        #[arg(long, default_value = "aristotle")]
        backend: String,
    },
    /// Poll an in-flight proof job (sparse polling for external provers).
    ProofPoll {
        job: String,
    },
    /// Cancel a proof job.
    ProofCancel {
        job: String,
    },
    /// Fetch the ProofResult for a terminal proof job.
    ProofResult {
        job: String,
    },
    /// List proof jobs for a project.
    ProofJobs {
        project: String,
        #[arg(default_value_t = 30)]
        limit: usize,
    },
    /// Start an AttemptRun (FLARE-style) with artifact directory.
    AttemptStart {
        project: String,
        #[arg(long)]
        node: Option<String>,
        /// JSON input: `{"statement":"...","theorem_name":"...","backend":"aristotle"}`.
        request: String,
    },
    /// Cancel a running AttemptRun.
    AttemptCancel {
        attempt: String,
    },
    /// Get AttemptRun result (polls linked proof job if still running).
    AttemptResult {
        attempt: String,
    },
    /// Drive an AttemptRun to completion (mock-friendly).
    AttemptRun {
        attempt: String,
        #[arg(default_value_t = 8)]
        max_polls: u32,
    },
    /// Run the conjecture engine: propose, falsify, and graduate only
    /// live-verified conjectures into the project's lemma library.
    Conjecture {
        project: String,
    },
    /// Detect undefined symbols in a statement and propose (advisory only)
    /// candidate definitions. Nothing is admitted to the library.
    DefineSynth {
        project: String,
        statement: String,
    },
    /// Evolve a marked proof sketch through the evolutionary loop, accepting a
    /// variant only when a live backend gate closes it.
    EvolveSketch {
        project: String,
        statement: String,
        /// The sketch template with EVOLVE-BLOCK markers.
        template: String,
    },
    /// Two-mode (fast vs chain-of-thought) formalization of an informal
    /// statement. Advisory: the screen is a heuristic, nothing is certified.
    FormalizeModes {
        project: String,
        informal: String,
    },
    /// Export library skills as a formal file, re-verifying each through the
    /// live gate first. Offline (no live gate) this exports nothing.
    MathlibExport {
        project: String,
        /// `lean` | `rocq` (`coq`) | `isabelle`.
        #[arg(long, default_value = "lean")]
        system: String,
    },
    /// Apply a proven method across a family of related problems, proving each
    /// through the portfolio gate. JSON: a `TransferSpec`.
    MethodTransfer {
        request: String,
    },
    /// Inverse-method (forward saturation) proof search. Records an unverified
    /// search outcome as node evidence. JSON:
    /// `{"node":"...","axioms":[...],"goal":"...","rules":[...]}`.
    InverseMethod {
        project: String,
        request: String,
    },
    /// Model-elimination refutation search. Records an unverified search
    /// outcome. JSON: `{"node":"...","clauses":[...],"max_bound":N}`.
    ModelElim {
        project: String,
        request: String,
    },
    /// Discovery-game (residual-reduction) search. JSON:
    /// `{"basis":[[..]],"root":[..],"seed":N}`. Result is a candidate, not a
    /// proof.
    DiscoverySearch {
        request: String,
        #[arg(long)]
        project: Option<String>,
    },
    /// SKEST speculative-ensemble search over a supplied goal graph. JSON:
    /// `{"closed_goals":[...],"edges":[{"from","tactic","prior","to"}],"root":"...","seed":N}`.
    SkestSearch {
        request: String,
        #[arg(long)]
        project: Option<String>,
    },
    /// Project a search DAG into its MCGS regression tree. JSON: a `DagView`.
    DagProject {
        project: String,
        request: String,
    },
    /// Mine Bradley-Terry critic preference pairs from verified winning paths
    /// only. JSON: an array of `CriticBranch`.
    PreferencePairs {
        project: String,
        request: String,
    },
    /// Independently re-check a serialized proof log; reports absent / empty /
    /// unparseable / rejected / checked distinctly.
    ProofLogCheck {
        file: PathBuf,
    },
    /// Report search telemetry (proof-length, diversity, round-over-round).
    /// JSON: `{"proofs":[...],"rounds":[[...]]}`.
    SearchTelemetry {
        request: String,
    },
    /// Run the multi-alpha best-first sweep over a statement, optionally
    /// shrinking the found proof behind a real checker re-check.
    ///
    /// A search result is a CANDIDATE. Only the optional minimize pass, which
    /// runs the full gate, confirms that a tactic sequence closes the goal.
    AlphaSweep {
        statement: String,
        #[arg(long)]
        project: Option<String>,
        /// `lean` | `rocq` (`coq`) | `isabelle`.
        #[arg(long, default_value = "lean")]
        system: String,
        /// Comma-separated length-normalization exponents.
        #[arg(long, default_value = "0.0,0.5,1.0")]
        alphas: String,
        #[arg(long, default_value_t = 200)]
        budget: usize,
        /// Shrink the proof behind a real gate re-check. Costs prover calls.
        #[arg(long)]
        minimize: bool,
        /// Weight on the state-value critic in the frontier score, in nats of
        /// log-probability per unit of critic value, PER STEP. It is scale-free
        /// because the critic is accumulated in the numerator and divided by the
        /// same length normalization as the policy, so one number means the same
        /// thing at every depth. 0.0 (the default) is byte-identical to no
        /// critic: no CriticScorer is constructed and the score branch evaluates
        /// the pre-seam expression.
        #[arg(long, default_value_t = 0.0)]
        critic_weight: f64,
    },
    /// Consult a Wolfram Engine or Wolfram|Alpha as an UNTRUSTED oracle.
    ///
    /// Nothing here certifies anything. A generated certificate is only returned
    /// when one of our own checkers accepts it, and a proposed counterexample
    /// only after we re-verify it in exact arithmetic. Both are opt-in and both
    /// report a clean "unavailable" when nothing is configured.
    Wolfram {
        /// `probe` | `evaluate` | `alpha` | `recognize` | `falsify` | `cert`
        op: String,
        /// Wolfram Language source for `evaluate`, the query text for `alpha`
        /// and `recognize`, or a JSON request for `falsify` and `cert`.
        input: String,
        /// Certificate kind for `cert`: `sos` | `nullstellensatz` | `sturm`.
        #[arg(long, default_value = "sos")]
        kind: String,
    },
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    // A subprocess bridge must use the same Store selected by `mcp`, rather
    // than re-resolving a potentially different default configuration.
    let database = match &cli.command {
        Command::Api {
            database: Some(database),
            ..
        } => database,
        _ => &config.database,
    };
    let store = Store::open(database)?;
    let provider: Box<dyn ModelProvider> = match &config.model_command {
        Some(c) => Box::new(CommandProvider::new(c)),
        None => Box::new(OfflineProvider),
    };
    match cli.command {
        Command::Init => {
            config.initialize()?;
            print_value(
                cli.json,
                &serde_json::json!({"initialized":true,"config":config}),
            )?
        }
        Command::Doctor => print_value(true, &capability_report(&config))?,
        Command::Mcp => {
            // Launch the MCP stdio server: it speaks JSON-RPC over inherited
            // stdin/stdout, so an MCP client drives the Python tool workers.
            // Give it an explicit, narrow Rust API bridge context. Python
            // invokes this binary for Store-backed meta-tools; it never opens
            // or writes SQLite directly.
            let python = tools::python_command()
                .ok_or_else(|| anyhow::anyhow!("no python interpreter found"))?;
            let bootstrap = tools::python_bootstrap("mcp_server");
            let executable = std::env::current_exe()?;
            let database = absolute_path(&config.database)?;
            let mut command = std::process::Command::new(python);
            command
                .args(["-E", "-c", &bootstrap])
                .env("THEOREMATA_MCP_API_COMMAND", executable)
                .env("THEOREMATA_MCP_DATABASE", database);
            if let Some(path) = cli.config.as_deref() {
                command.env("THEOREMATA_MCP_CONFIG", absolute_path(path)?);
            }
            let status = command.status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
        Command::New { name, theorem } => {
            print_value(cli.json, &store.create_project(&name, &theorem)?)?
        }
        Command::Projects => print_value(cli.json, &store.list_projects()?)?,
        Command::Status { project } => print_value(cli.json, &store.project(&project)?)?,
        Command::Graph { project } => print_value(true, &store.export(&project)?)?,
        Command::AddNode {
            project,
            kind,
            title,
            statement,
        } => print_value(
            cli.json,
            &store.add_node(&project, kind, &title, &statement, "user")?,
        )?,
        Command::Link {
            project,
            source,
            target,
            kind,
        } => {
            store.add_edge(&project, &source, &target, kind.parse::<EdgeKind>()?)?;
            print_value(cli.json, &serde_json::json!({"linked":true}))?
        }
        Command::SetStatus {
            project,
            node,
            status,
        } => {
            // Trust boundary shared with the versioned API: a CLI command must
            // not be able to forge a machine-checked proof status without the
            // verifier's evidence-bearing certification path.
            if status == NodeStatus::FormallyVerified {
                anyhow::bail!(
                    "formally_verified is set only by the verification pipeline; \
                     the CLI cannot grant it directly"
                );
            }
            store.set_node_status(&project, &node, status, "user")?;
            print_value(cli.json, &serde_json::json!({"updated":true}))?
        }
        Command::Chat { project } => tui::run(&store, &config, provider.as_ref(), &project)?,
        Command::Send { project, message } => {
            let mut on_event = |event: model::ModelStreamEvent| {
                if !cli.json {
                    if let model::ModelStreamEvent::Delta { text } = event {
                        print!("{text}");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                    }
                }
            };
            let engine = chat::ChatEngine {
                store: &store,
                provider: provider.as_ref(),
            };
            let reply = engine.send_stream(&project, &message, &mut on_event)?;
            let auto_approved =
                engine.resolve_auto_approvals(&project, config.auto_approve_safe)?;
            if !cli.json {
                println!();
            }
            print_value(
                cli.json,
                &serde_json::json!({"reply":reply,"auto_approved":auto_approved}),
            )?
        }
        Command::Proposals { project, all } => {
            print_value(true, &store.proposals(&project, !all)?)?
        }
        Command::Approve { project, proposal } => {
            chat::ChatEngine {
                store: &store,
                provider: provider.as_ref(),
            }
            .approve(&project, &proposal)?;
            print_value(cli.json, &serde_json::json!({"approved":proposal}))?
        }
        Command::Reject {
            project,
            proposal,
            note,
        } => {
            chat::ChatEngine {
                store: &store,
                provider: provider.as_ref(),
            }
            .reject(&project, &proposal, &note)?;
            print_value(cli.json, &serde_json::json!({"rejected":proposal}))?
        }
        Command::Events { project, limit } => {
            print_value(cli.json, &store.events(&project, limit)?)?
        }
        Command::Attempts { project, limit } => {
            print_value(cli.json, &store.attempts(&project, limit)?)?
        }
        Command::Export { project, output } => {
            fs::write(
                &output,
                serde_json::to_string_pretty(&store.export(&project)?)?,
            )?;
            print_value(cli.json, &serde_json::json!({"output":output}))?
        }
        Command::Search { query, limit } => print_value(
            true,
            &MathlibSearch::new(&config).run(serde_json::json!({"query":query,"limit":limit}))?,
        )?,
        Command::Compute { expression } => print_value(
            true,
            &PythonCheck::new().run(serde_json::json!({"expression":expression}))?,
        )?,
        Command::Falsify {
            variables,
            claim,
            assumptions,
            max_cases,
        } => {
            let variables: serde_json::Value = serde_json::from_str(&variables)?;
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"falsify","variables":variables,"claim":claim,
                    "assumptions":assumptions,"max_cases":max_cases
                }))?,
            )?
        }
        Command::Symbolic {
            operation,
            expression,
            variable,
        } => print_value(
            true,
            &PythonCheck::new().run(serde_json::json!({
                "tool":"symbolic","operation":operation,
                "expression":expression,"variable":variable
            }))?,
        )?,
        Command::Estimates => print_value(
            true,
            &PythonCheck::new().run(serde_json::json!({
                "tool":"estimates_capability","resources":config.resources
            }))?,
        )?,
        Command::Feasibility { constraints } => {
            let constraints: serde_json::Value = serde_json::from_str(&constraints)?;
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"feasibility","constraints":constraints
                }))?,
            )?
        }
        Command::Asymptotic { request } => {
            let mut request: serde_json::Value = serde_json::from_str(&request)?;
            let op = request["op"]
                .as_str()
                .unwrap_or("asymptotic_feasibility")
                .to_string();
            request["tool"] = serde_json::json!(op);
            print_value(true, &PythonCheck::new().run(request)?)?
        }
        Command::Grade { request } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"grader","request":request
                }))?,
            )?
        }
        Command::Stages { request } => {
            let mut request: serde_json::Value = serde_json::from_str(&request)?;
            request["tool"] = serde_json::json!("stages");
            print_value(true, &PythonCheck::new().run(request)?)?
        }
        Command::Imports {
            query,
            module,
            substring,
            limit,
        } => {
            let root = config.resources.join("mathlib4-master/mathlib4-master");
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"mathlib_index","root":root,"query":query,
                    "module":module,"substring":substring,"limit":limit
                }))?,
            )?
        }
        Command::Decls {
            query,
            substring,
            kind,
            limit,
            mathlib,
        } => {
            let (root, imports) = if mathlib {
                (
                    Some(config.resources.join("mathlib4-master/mathlib4-master")),
                    vec!["Mathlib".to_string()],
                )
            } else {
                (None, vec!["Init".to_string()])
            };
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"decl_index","root":root,"imports":imports,
                    "query":query,"kind":kind,"substring":substring,"limit":limit
                }))?,
            )?
        }
        Command::Heads {
            query,
            head,
            pattern,
            limit,
            mathlib,
        } => {
            let (root, imports) = if mathlib {
                (
                    Some(config.resources.join("mathlib4-master/mathlib4-master")),
                    vec!["Mathlib".to_string()],
                )
            } else {
                (None, vec!["Init".to_string()])
            };
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"head_index","root":root,"imports":imports,
                    "query":query,"head":head,"pattern":pattern,"limit":limit
                }))?,
            )?
        }
        Command::Soundness { file } => {
            let text = fs::read_to_string(&file)?;
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"lean_soundness","text":text
                }))?,
            )?
        }
        Command::Axioms { file, theorem } => {
            let source = fs::read_to_string(&file)?;
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"check_axioms","source":source,"theorem":theorem,
                    "root":config.lean_project
                }))?,
            )?
        }
        Command::Schedule { project } => {
            let nodes = store.nodes(&project)?;
            let edges = store.edges(&project)?;
            print_value(true, &scheduler::plan(&nodes, &edges))?
        }
        Command::Paranoia { theorem } => print_value(
            true,
            &LeanParanoia::new(&config).run(serde_json::json!({ "theorem": theorem }))?,
        )?,
        Command::Workspace { request } => {
            let mut request: serde_json::Value = serde_json::from_str(&request)?;
            let tool = if request["op"].as_str() == Some("place") {
                "lean_workspace_place"
            } else {
                "lean_workspace_scaffold"
            };
            request["tool"] = serde_json::json!(tool);
            print_value(true, &PythonCheck::new().run(request)?)?
        }
        Command::LeanWarm { request } => {
            let mut request: serde_json::Value = serde_json::from_str(&request)?;
            request["tool"] = serde_json::json!("lean_warm");
            print_value(true, &PythonCheck::new().run(request)?)?
        }
        Command::Tool { request } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            print_value(true, &PythonCheck::new().run(request)?)?
        }
        Command::Api { request, .. } => println!("{}", api::handle_json(&store, &request)),
        Command::BlueprintExport { project, out_dir } => {
            let export = blueprint::export(&store, &project)?;
            std::fs::create_dir_all(&out_dir)?;
            std::fs::write(out_dir.join("content.tex"), &export.content_tex)?;
            let decls_path = blueprint::lean_decls_path(&out_dir);
            if let Some(parent) = decls_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let decls_count = export
                .lean_decls
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count();
            std::fs::write(&decls_path, &export.lean_decls)?;
            print_value(
                cli.json,
                &serde_json::json!({
                    "content_tex": out_dir.join("content.tex"),
                    "lean_decls": decls_path,
                    "decls": decls_count,
                }),
            )?
        }
        Command::BlueprintImport { project, file } => {
            let tex = std::fs::read_to_string(&file)?;
            print_value(true, &blueprint::import(&store, &project, &tex)?)?
        }
        Command::BlueprintRun {
            project,
            file,
            systems,
        } => {
            store.project(&project)?;
            let systems: Vec<prover::formal::FormalSystem> = match &systems {
                Some(s) => s
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::parse)
                    .collect::<Result<_>>()?,
                None => Vec::new(),
            };
            let tex = std::fs::read_to_string(&file)?;
            let run = blueprint_run::BlueprintRun::from_tex(&tex)?;
            let generator = sketch::WholeStatementGenerator;
            let hole_prover = sketch::PortfolioHoleProver {
                store: &store,
                config: &config,
                provider: provider.as_ref(),
                systems,
            };
            let adapter = blueprint_run::SketchObligationProver {
                store: &store,
                project_id: &project,
                generator: &generator,
                prover: &hole_prover,
                provider: provider.as_ref(),
                gate_enabled: certification::gate_enabled(),
            };
            print_value(true, &run.drive(&adapter)?)?
        }
        Command::Checkdecls {
            workspace,
            manifest,
        } => {
            let manifest = manifest.unwrap_or_else(|| blueprint::lean_decls_path(&workspace));
            let decls: Vec<String> = std::fs::read_to_string(&manifest)
                .unwrap_or_default()
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_owned)
                .collect();
            print_value(true, &blueprint::check_decls(&workspace, &decls)?)?
        }
        Command::Agent { project } => print_value(
            true,
            &agent::AgentLoop {
                store: &store,
                config: &config,
                provider: provider.as_ref(),
            }
            .run(&project)?,
        )?,
        Command::Harden { project, node } => {
            let target = store
                .nodes(&project)?
                .into_iter()
                .find(|n| n.id == node)
                .ok_or_else(|| anyhow::anyhow!("node not found: {node}"))?;
            let source = target.formal_statement.clone().unwrap_or_default();
            let module = format!(
                "N{}",
                node.replace('-', "").chars().take(8).collect::<String>()
            );
            print_value(
                true,
                &hardening::harden(&store, &config, &project, &node, &module, &source)?,
            )?
        }
        Command::Research { project } => print_value(
            true,
            &research::ResearchEngine {
                store: &store,
                provider: provider.as_ref(),
            }
            .run(&project)?,
        )?,
        Command::Critique { project } => print_value(
            true,
            &critic::Critic {
                store: &store,
                provider: provider.as_ref(),
            }
            .critique(&project)?,
        )?,
        Command::Consolidate { project } => print_value(
            true,
            &consolidate::Consolidator {
                store: &store,
                provider: provider.as_ref(),
            }
            .run(&project)?,
        )?,
        Command::Team { project } => {
            let batches = team::parallel_batches(&store, &project)?;
            let workers = team::Team {
                db_path: config.database.clone(),
                max_workers: 4,
            };
            let mut outcomes = Vec::new();
            for batch in &batches {
                outcomes.extend(workers.process_batch(&project, batch)?);
            }
            print_value(true, &outcomes)?
        }
        Command::Trace { project, limit } => print_value(
            true,
            &observe::Observer { store: &store }.trace(&project, limit)?,
        )?,
        Command::Metrics { project } => print_value(
            true,
            &observe::Observer { store: &store }.metrics(&project)?,
        )?,
        Command::Replay { project, run } => print_value(
            true,
            &observe::Observer { store: &store }.replay(&project, run.as_deref())?,
        )?,
        Command::Sweep {
            project,
            limit,
            reelaborate_preamble,
            max_reelaborate,
        } => print_value(
            true,
            &reason::proving::staleness_sweep::sweep_with_options(
                &store,
                &config,
                project.as_deref(),
                limit,
                &reason::proving::staleness_sweep::SweepOptions {
                    reelaboration_preamble: reelaborate_preamble,
                    max_reelaborations: max_reelaborate,
                },
            )?,
        )?,
        Command::Tactics { goal } => print_value(
            true,
            &mcts::TacticMcts {
                provider: provider.as_ref(),
            }
            .propose_tactics(&goal, 5)?,
        )?,
        Command::Strategies { goal } => print_value(
            true,
            &sampler::verbalized_sample(provider.as_ref(), &goal, 4)?,
        )?,
        Command::SftExport { project } => {
            let records: Vec<serde_json::Value> = store
                .nodes(&project)?
                .into_iter()
                .filter(|n| {
                    n.status == NodeStatus::FormallyVerified && n.formal_statement.is_some()
                })
                .map(|n| {
                    serde_json::json!({
                        "goal": n.statement, "proof": n.formal_statement,
                        "verified": true, "axioms_ok": true
                    })
                })
                .collect();
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"sft_export","request":{"op":"star_dataset","records":records}
                }))?,
            )?
        }
        Command::Evolve { request } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({"tool":"evolve","request":request}))?,
            )?
        }
        Command::Grpo { request } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({"tool":"grpo","request":request}))?,
            )?
        }
        Command::Route { project } => {
            let nodes = store.nodes(&project)?;
            let model_ready = config.model_command.is_some();
            let tools = router::ToolAvailability {
                python: PythonCheck::new().available(),
                lean: LeanCheck::new(&config).available(),
                formal_verifier: prover::formal::backend_for(&config, config.target_system, false)
                    .available(),
                mathlib_search: MathlibSearch::new(&config).available(),
                model: model_ready,
                external_prover: proof_job::any_prover_available(&config, model_ready),
            };
            let routes: Vec<_> = nodes
                .iter()
                .filter(|n| matches!(n.status, NodeStatus::Proposed | NodeStatus::Active))
                .map(|n| {
                    let signals = router::NodeSignals {
                        has_formal_statement: n.formal_statement.is_some(),
                        ..Default::default()
                    };
                    serde_json::json!({
                        "node": n.id, "title": n.title,
                        "route": router::route(n, &signals, &tools, config.max_iterations)
                    })
                })
                .collect();
            print_value(true, &routes)?
        }
        Command::Lemma {
            project,
            node,
            name,
        } => print_value(true, &store.extract_lemma(&project, &node, &name)?)?,
        Command::Lemmas { project } => print_value(true, &store.lemmas(&project)?)?,
        Command::Rehash { project } => {
            store.recompute_all_hashes(&project)?;
            print_value(cli.json, &serde_json::json!({"rehashed": project}))?
        }
        Command::Eval { request } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({"tool":"eval","request":request}))?,
            )?
        }
        Command::Retrieve {
            query,
            mathlib,
            limit,
        } => {
            let (root, imports) = if mathlib {
                (
                    Some(config.resources.join("mathlib4-master/mathlib4-master")),
                    vec!["Mathlib".to_string()],
                )
            } else {
                (None, vec!["Init".to_string()])
            };
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"retrieve","root":root,"imports":imports,
                    "query":query,"limit":limit,"op":"retrieve"
                }))?,
            )?
        }
        Command::FormalProve { system, statement } => {
            let system: prover::formal::FormalSystem = system.parse()?;
            let (code, report) = formal_generate::generate_and_verify(
                &store,
                &config,
                provider.as_ref(),
                system,
                &statement,
            )?;
            print_value(
                true,
                &serde_json::json!({
                    "system": system.as_str(),
                    "verified": report.lexically_verified,
                    "code": code,
                    "report": report,
                }),
            )?
        }
        Command::HammerProve { system, goal } => {
            let system: prover::formal::FormalSystem = system.parse()?;
            // Ask the hammer worker to FIND a tactic and assemble a native proof.
            let assembled = formal_generate::hammer_prove(&config, system, &goal);
            match assembled {
                Some(code) => {
                    // Verify through the live gate when its toolchain is present,
                    // else the mock backend (whose source scan still runs for real).
                    let live = prover::formal::backend_for(&config, system, false);
                    let used_live = !config.prover_mock && live.available();
                    let backend = if used_live {
                        live
                    } else {
                        prover::formal::backend_for(&config, system, true)
                    };
                    let report = backend.verify(&config, &code, &goal)?;
                    print_value(
                        true,
                        &serde_json::json!({
                            "system": system.as_str(),
                            "goal": goal,
                            "path": "hammer",
                            "backend": if used_live { "live" } else { "mock" },
                            "verified": report.lexically_verified,
                            "code": code,
                            "report": report,
                        }),
                    )?
                }
                None => print_value(
                    true,
                    &serde_json::json!({
                        "system": system.as_str(),
                        "goal": goal,
                        "path": "hammer",
                        "verified": false,
                        "code": serde_json::Value::Null,
                        "message": "hammer produced no reconstruction (worker unavailable or no proof found)",
                    }),
                )?,
            }
        }
        Command::PortfolioProve { statement, systems } => {
            let systems: Vec<prover::formal::FormalSystem> = match &systems {
                Some(s) => s
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::parse)
                    .collect::<Result<_>>()?,
                None => Vec::new(),
            };
            let result = portfolio::portfolio_prove(
                &store,
                &config,
                provider.as_ref(),
                &statement,
                &systems,
            )?;
            print_value(
                true,
                &serde_json::json!({
                    "statement": result.statement,
                    "winner": result.winner.map(|s| s.as_str()),
                    "any_verified": result.any_verified,
                    "per_system": result.per_system,
                }),
            )?
        }
        Command::ProofSubmit {
            project,
            statement,
            node,
            theorem,
            backend,
        } => {
            store.project(&project)?;
            let mut task = aristotle::build_task(
                Some(project.clone()),
                node.clone(),
                &statement,
                &theorem,
                &config,
            );
            task.backend = backend;
            print_value(true, &proof_job::submit(&store, &config, task, None)?)?
        }
        Command::ProofPoll { job } => {
            print_value(true, &proof_job::poll(&store, &config, &job, None)?)?
        }
        Command::ProofCancel { job } => print_value(true, &proof_job::cancel(&store, &job)?)?,
        Command::ProofResult { job } => print_value(true, &proof_job::result(&store, &job)?)?,
        Command::ProofJobs { project, limit } => {
            print_value(true, &store.list_proof_jobs(&project, limit)?)?
        }
        Command::AttemptStart {
            project,
            node,
            request,
        } => {
            let input: serde_json::Value = serde_json::from_str(&request)?;
            print_value(
                true,
                &attempt_run::start(&store, &config, &project, node.as_deref(), input)?,
            )?
        }
        Command::AttemptCancel { attempt } => {
            print_value(true, &attempt_run::cancel(&store, &attempt)?)?
        }
        Command::AttemptResult { attempt } => {
            print_value(true, &attempt_run::result(&store, &config, &attempt, None)?)?
        }
        Command::AttemptRun { attempt, max_polls } => {
            std::env::set_var("THEOREMATA_ARISTOTLE_MOCK", "1");
            print_value(
                true,
                &attempt_run::run_to_completion(&store, &config, &attempt, max_polls, None)?,
            )?
        }
        Command::Lean { file } => print_value(
            true,
            &LeanCheck::new(&config).run(serde_json::json!({ "file": file }))?,
        )?,
        Command::Conjecture { project } => print_value(
            true,
            &conjecture_engine::run(&store, &config, provider.as_ref(), &project)?,
        )?,
        Command::DefineSynth { project, statement } => print_value(
            true,
            &definition_synthesis::synthesize(&store, provider.as_ref(), &project, &statement)?,
        )?,
        Command::EvolveSketch {
            project,
            statement,
            template,
        } => print_value(
            true,
            &evolve_sketch::evolve_proof_sketch(
                &store,
                &config,
                provider.as_ref(),
                &project,
                &statement,
                &template,
                &evolve_sketch::EvolveConfig::default(),
            )?,
        )?,
        Command::FormalizeModes { project, informal } => print_value(
            true,
            &formalize_modes::run_two_mode_formalization(
                &store,
                &config,
                provider.as_ref(),
                &project,
                &informal,
                &formalize_modes::TwoModeConfig::default(),
            )?,
        )?,
        Command::MathlibExport { project, system } => {
            let system = parse_system(&system)?;
            print_value(
                true,
                &mathlib_export::export_verified(
                    &store,
                    &config,
                    &project,
                    system,
                    &mathlib_export::ExportConfig::default(),
                )?,
            )?
        }
        Command::MethodTransfer { request } => {
            let spec: method_transfer::TransferSpec = serde_json::from_str(&request)?;
            print_value(
                true,
                &method_transfer::transfer(&store, &config, provider.as_ref(), &spec)?,
            )?
        }
        Command::InverseMethod { project, request } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            let node = request
                .get("node")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let axioms = json_string_array(&request, "axioms");
            let goal = request
                .get("goal")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let rules = json_string_array(&request, "rules");
            print_value(
                true,
                &inverse_method::saturate_spec(
                    &store,
                    &project,
                    node,
                    &axioms,
                    goal,
                    &rules,
                    &inverse_method::SaturationConfig::default(),
                )?,
            )?
        }
        Command::ModelElim { project, request } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            let node = request
                .get("node")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let clauses = json_string_array(&request, "clauses");
            let max_bound = request
                .get("max_bound")
                .and_then(|v| v.as_u64())
                .unwrap_or(1000) as usize;
            print_value(
                true,
                &model_elimination::refute_clauses(&store, &project, node, &clauses, max_bound)?,
            )?
        }
        Command::DiscoverySearch { request, project } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            let basis: Vec<Vec<i64>> = request
                .get("basis")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let root: Vec<i64> = request
                .get("root")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let seed = request.get("seed").and_then(|v| v.as_u64()).unwrap_or(0);
            print_value(
                true,
                &discovery_game::run_discovery_search(
                    &store,
                    project.as_deref(),
                    basis,
                    root,
                    discovery_game::DiscoveryConfig::default(),
                    seed,
                )?,
            )?
        }
        Command::SkestSearch { request, project } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            let closed_goals = json_string_array(&request, "closed_goals");
            let edges: Vec<skest::GraphEdge> = request
                .get("edges")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            let root = request
                .get("root")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let seed = request.get("seed").and_then(|v| v.as_u64()).unwrap_or(0);
            print_value(
                true,
                &skest::run_skest_search(
                    &store,
                    project.as_deref(),
                    closed_goals,
                    edges,
                    root,
                    skest::SkestConfig::default(),
                    seed,
                )?,
            )?
        }
        Command::DagProject { project, request } => {
            let dag: dag_projection::DagView = serde_json::from_str(&request)?;
            print_value(
                true,
                &dag_projection::project_search_dag(&store, &project, &dag)?,
            )?
        }
        Command::PreferencePairs { project, request } => {
            let branches: Vec<preference_pairs::CriticBranch> = serde_json::from_str(&request)?;
            print_value(
                true,
                &preference_pairs::mine_critic_pairs(&store, &project, &branches)?,
            )?
        }
        Command::ProofLogCheck { file } => print_value(true, &proof_log::check_log_file(&file)?)?,
        Command::AlphaSweep {
            statement,
            project,
            system,
            alphas,
            budget,
            minimize,
            critic_weight,
        } => {
            let system = parse_system(&system)?;
            let alphas: Vec<f64> = alphas
                .split(',')
                .filter_map(|a| a.trim().parse::<f64>().ok())
                .collect();
            if alphas.is_empty() {
                return Err(anyhow::anyhow!("no parseable alphas in --alphas"));
            }
            print_value(
                true,
                &hybrid_search::run_alpha_sweep_search(
                    &store,
                    &config,
                    provider.as_ref(),
                    project.as_deref(),
                    &statement,
                    system,
                    &alphas,
                    budget,
                    minimize,
                    critic_weight,
                )?,
            )?
        }
        Command::Wolfram { op, input, kind } => {
            // One command over the five oracle tools, because they share a trust
            // posture: every one of them is untrusted, and grouping them keeps
            // that visible rather than scattering them among the verbs that do
            // certify something.
            let request = match op.as_str() {
                "probe" => serde_json::json!({"tool": "wolfram_link", "op": "available"}),
                "evaluate" => {
                    serde_json::json!({"tool": "wolfram_link", "op": "evaluate", "code": input})
                }
                "alpha" => {
                    serde_json::json!({"tool": "wolfram_alpha", "op": "query", "input": input})
                }
                "recognize" => {
                    serde_json::json!({"tool": "wolfram_recognizer", "op": "recognize", "text": input})
                }
                // falsify and cert take a JSON payload, since their inputs are
                // structured (variables, claim, polynomial) rather than one string.
                "falsify" => {
                    let mut payload: serde_json::Value = serde_json::from_str(&input)?;
                    payload["tool"] = serde_json::json!("wolfram_falsify");
                    payload["op"] = serde_json::json!("falsify");
                    payload
                }
                "cert" => {
                    let mut payload: serde_json::Value = serde_json::from_str(&input)?;
                    payload["tool"] = serde_json::json!("wolfram_cert");
                    payload["op"] = serde_json::json!(kind);
                    payload
                }
                other => {
                    return Err(anyhow::anyhow!(
                        "unknown wolfram op: {other} (expected probe, evaluate, alpha, \
                         recognize, falsify, or cert)"
                    ))
                }
            };
            print_value(true, &PythonCheck::new().run(request)?)?
        }
        Command::SearchTelemetry { request } => {
            let request: serde_json::Value = serde_json::from_str(&request)?;
            print_value(true, &search_telemetry::report(&request)?)?
        }
    }
    Ok(())
}

/// Parse a `String` array field out of a request object, empty when absent.
fn json_string_array(request: &serde_json::Value, key: &str) -> Vec<String> {
    request
        .get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolve a system name to a [`FormalSystem`], accepting `coq` for Rocq.
fn parse_system(name: &str) -> Result<prover::formal::FormalSystem> {
    use prover::formal::FormalSystem;
    match name.trim().to_ascii_lowercase().as_str() {
        "lean" => Ok(FormalSystem::Lean),
        "rocq" | "coq" => Ok(FormalSystem::Rocq),
        "isabelle" => Ok(FormalSystem::Isabelle),
        other => Err(anyhow::anyhow!("unknown formal system: {other}")),
    }
}

/// Resolve a caller-selected file path once before passing it across a process
/// boundary. This does not canonicalize, so SQLite's normal path semantics and
/// a not-yet-created database file remain intact.
fn absolute_path(path: &std::path::Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}
fn print_value<T: serde::Serialize + std::fmt::Debug>(json: bool, value: &T) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?)
    } else {
        println!("{value:#?}")
    };
    Ok(())
}
