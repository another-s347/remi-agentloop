//! Deep agent CLI example
//!
//! Config is loaded from `deep-agent.toml` (run with `--init` to generate one)
//! or from environment variables as fallback.
//!
//! Run:
//!   cargo run -p remi-deepagent --example deep_agent_cli
//!
//! Or with an initial task:
//!   cargo run ... -- "Create a hello-world Rust project"
//!
//! First-time setup:
//!   cargo run ... -- --init

use futures::StreamExt;
use remi_deepagent::{DeepAgentBuilder, DeepAgentConfig, DeepAgentEvent, SkillEvent, TodoEvent};
use remi_model::OpenAIClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── --init ───────────────────────────────────────────────────────────────
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--init") {
        let force = args.iter().any(|a| a == "--force");
        DeepAgentConfig::write_example(force)?;
        println!("Edit deep-agent.toml and re-run.");
        return Ok(());
    }

    // ── Config ───────────────────────────────────────────────────────────────
    let cfg = DeepAgentConfig::load()
        .map_err(|e| format!("Config error: {e}"))?;
    cfg.require_api_key().map_err(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    // ── Model setup ──────────────────────────────────────────────────────────
    let mut oai = OpenAIClient::new(cfg.model.api_key.clone())
        .with_model(&cfg.model.model);
    if let Some(url) = &cfg.model.base_url {
        oai = oai.with_base_url(url.clone());
    }

    // ── Build deep agent ─────────────────────────────────────────────────────
    let agent = cfg.apply_to_builder(DeepAgentBuilder::new(oai)).build();

    // ── Task ─────────────────────────────────────────────────────────────────
    let task = args.iter().skip(1)
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");

    let task = if task.is_empty() {
        "Create a hello-world Rust project in /tmp/deep-agent-demo using bash. \
         Track your work with todos (add one todo per step: mkdir, write Cargo.toml, \
         write main.rs, cargo build). \
         After building, save a reusable 'create-rust-hello-world' skill. \
         Finally list the directory contents to confirm the build succeeded."
            .to_string()
    } else {
        task
    };

    println!("╔══════════════════════════════════════════╗");
    println!("║          remi-deepagent demo              ║");
    println!("╚══════════════════════════════════════════╝");
    println!("Task: {task}");
    println!("{}", "─".repeat(50));

    // ── Stream events ────────────────────────────────────────────────────────
    let stream = agent.chat(&task).await?;
    let mut stream = std::pin::pin!(stream);

    let mut todo_count = 0u64;
    let mut skill_count = 0u64;

    while let Some(ev) = stream.next().await {
        match &ev {
            // ── Agent events ──────────────────────────────────────────────────
            DeepAgentEvent::Agent(ae) => match ae {
                remi_core::types::AgentEvent::RunStart { thread_id, run_id, .. } => {
                    println!("\n[run start  thread={thread_id} run={run_id}]");
                }
                remi_core::types::AgentEvent::TurnStart { turn } => {
                    println!("\n── turn {turn} ──────────────────────────────");
                }
                remi_core::types::AgentEvent::TextDelta(t) => print!("{t}"),
                remi_core::types::AgentEvent::ToolCallStart { name, .. } => {
                    println!();
                    print!("  ▶ {name}(");
                }
                remi_core::types::AgentEvent::ToolCallArgumentsDelta { delta, .. } => {
                    print!("{delta}");
                }
                remi_core::types::AgentEvent::ToolDelta { delta, .. } => {
                    // progress delta from a streaming tool
                    print!("{delta}");
                }
                remi_core::types::AgentEvent::ToolResult { name, result, .. } => {
                    println!(")");
                    // Don't print huge results (FileBackedRegistry already truncated them)
                    let preview = if result.len() > 200 {
                        format!("{}… [{} bytes]", &result[..200], result.len())
                    } else {
                        result.clone()
                    };
                    println!("  ◀ {name}: {preview}");
                }
                remi_core::types::AgentEvent::Usage { prompt_tokens, completion_tokens } => {
                    println!("\n[usage  prompt={prompt_tokens}  completion={completion_tokens}]");
                }
                remi_core::types::AgentEvent::Done => {
                    println!("\n\n✅  Agent finished.");
                }
                remi_core::types::AgentEvent::Error(e) => {
                    println!("\n❌  Error: {e}");
                }
                _ => {}
            },

            // ── Todo events ───────────────────────────────────────────────────
            DeepAgentEvent::Todo(te) => match te {
                TodoEvent::Added { id, content } => {
                    todo_count += 1;
                    println!("\n  📋 TODO added  #{id}: {content}");
                }
                TodoEvent::Completed { id } => {
                    println!("\n  ✅ TODO done   #{id}");
                }
                TodoEvent::Updated { id, content } => {
                    println!("\n  ✏️  TODO updated #{id}: {content}");
                }
                TodoEvent::Removed { id } => {
                    println!("\n  🗑️  TODO removed #{id}");
                }
            },

            // ── Skill events ──────────────────────────────────────────────────
            DeepAgentEvent::Skill(se) => match se {
                SkillEvent::Saved { name, path } => {
                    skill_count += 1;
                    println!("\n  💾 SKILL saved  '{name}' → {path}");
                }
                SkillEvent::Deleted { name } => {
                    println!("\n  🗑️  SKILL deleted '{name}'");
                }
            },
        }
    }

    println!("\n{}", "─".repeat(50));
    println!("Summary: {todo_count} todo(s) created, {skill_count} skill(s) saved.");

    Ok(())
}
