mod chat;
mod config;
mod db;
mod model;
mod provider;
mod retry;
mod tools;
mod tui;
mod workflow;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Config;
use db::Store;
use model::{EdgeKind, NodeKind, NodeStatus};
use provider::{CommandProvider, ModelProvider, OfflineProvider};
use std::{fs, path::PathBuf};
use tools::{capability_report, LeanCheck, MathlibSearch, PythonCheck, Tool};
use workflow::ResearchWorkflow;

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
    Run {
        project: String,
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
    Soundness {
        file: PathBuf,
    },
    Lean {
        file: PathBuf,
    },
}

fn main() -> Result<()> {
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
        Command::Run { project } => print_value(
            true,
            &ResearchWorkflow {
                store: &store,
                config: &config,
                provider: provider.as_ref(),
            }
            .run(&project)?,
        )?,
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
            let reply = chat::ChatEngine {
                store: &store,
                provider: provider.as_ref(),
            }
            .send_stream(&project, &message, &mut on_event)?;
            if !cli.json {
                println!();
            }
            print_value(cli.json, &serde_json::json!({"reply":reply}))?
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
        Command::Soundness { file } => {
            let text = fs::read_to_string(&file)?;
            print_value(
                true,
                &PythonCheck::new().run(serde_json::json!({
                    "tool":"lean_soundness","text":text
                }))?,
            )?
        }
        Command::Lean { file } => {
            print_value(true, &LeanCheck.run(serde_json::json!({"file":file}))?)?
        }
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
