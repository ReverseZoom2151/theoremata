mod api;
mod config;
#[path = "../components/prover/mod.rs"]
mod prover;
#[path = "../components/graph/mod.rs"]
mod graph;
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
pub use graph::{db, model, scheduler};
pub use reason::{
    agent, blueprint, blueprint_generate, blueprint_run, certification, chat, consolidate, critic,
    decompose, definition_synthesis, discovery_game, driver, evolve_sketch, falsification, fitness,
    formal_generate, formalize_portfolio, goal_cache, guard, inverse_method, library, mathlib_export,
    mcts, memory, method_transfer, minimize, observe, optimize, plan_history, portfolio,
    process_reward, progress, proof_pool, repair, research, retry, rewriting, router, sampler,
    sampling, sketch, skest, statement_validation, subsumption, symmetry_dedup, tactic_outcome,
    taint, team, ttc,
};
pub use prover::{
    aristotle, attempt_run, axiom_audit, exec, formal, goal_state, isabelle, lean, proof_job,
    proof_log, rocq,
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
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;
    let store = Store::open(&config.database)?;
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
            let python = tools::python_command()
                .ok_or_else(|| anyhow::anyhow!("no python interpreter found"))?;
            let bootstrap = tools::python_bootstrap("mcp_server");
            let status = std::process::Command::new(python)
                .args(["-E", "-c", &bootstrap])
                .status()?;
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
            store.set_node_status(&project, &node, status, "user")?;
            print_value(cli.json, &serde_json::json!({"updated":true}))?
        }
        Command::Chat { project } => tui::run(&store, provider.as_ref(), &project)?,
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
        Command::Api { request } => println!("{}", api::handle_json(&store, &request)),
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
            print_value(
                true,
                &proof_job::submit(&store, &config, task, None)?,
            )?
        }
        Command::ProofPoll { job } => {
            print_value(true, &proof_job::poll(&store, &config, &job, None)?)?
        }
        Command::ProofCancel { job } => {
            print_value(true, &proof_job::cancel(&store, &job)?)?
        }
        Command::ProofResult { job } => {
            print_value(true, &proof_job::result(&store, &job)?)?
        }
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
            print_value(
                true,
                &attempt_run::result(&store, &config, &attempt, None)?,
            )?
        }
        Command::AttemptRun { attempt, max_polls } => {
            std::env::set_var("THEOREMATA_ARISTOTLE_MOCK", "1");
            print_value(
                true,
                &attempt_run::run_to_completion(
                    &store,
                    &config,
                    &attempt,
                    max_polls,
                    None,
                )?,
            )?
        }
        Command::Lean { file } => print_value(
            true,
            &LeanCheck::new(&config).run(serde_json::json!({ "file": file }))?,
        )?,
    }
    Ok(())
}
fn print_value<T: serde::Serialize + std::fmt::Debug>(json: bool, value: &T) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?)
    } else {
        println!("{value:#?}")
    };
    Ok(())
}
