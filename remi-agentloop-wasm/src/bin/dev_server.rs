//! remi-dev-server — Hot-reloading WASM agent dev server
//!
//! Serves a WASM agent over HTTP SSE and watches source code for changes,
//! invoking `remi build` and hot-swapping the WASM module on every save.
//!
//! Architecture:
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │  AgentSlot = Arc<RwLock<Option<Arc<WasmAgentWithHttp>>>> │
//! │         ↑ reload writes          ↑ each request reads │
//! │                                                     │
//! │  HttpSseServer                  FileWatcher         │
//! │    POST /chat                     notify crate      │
//! │      ↓ clone agent Arc             ↓ .rs / Cargo.toml │
//! │      ↓ spawn thread             debounce 500 ms     │
//! │      ↓ run agent stream →       remi build          │
//! │      ↓ forward ProtocolEvents   → reload AgentSlot  │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! Usage:
//! ```sh
//! cargo run -p remi-agentloop-wasm --bin remi-dev-server --features dev-server \
//!     -- --agent examples/composable-calculator-agent --port 8080
//! ```

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use notify::{RecursiveMode, Watcher};
use tokio_stream::wrappers::ReceiverStream;

use remi_agentloop::agent::Agent;
use remi_agentloop::protocol::{ProtocolError, ProtocolEvent};
use remi_agentloop::transport::HttpSseServer;
use remi_agentloop::types::LoopInput;
use remi_agentloop_wasm::WasmAgentWithHttp;

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "remi-dev-server",
    about = "Hot-reloading WASM agent dev server over HTTP SSE.\n\
             Uses `remi build` to compile the agent on startup and on every source change."
)]
struct Args {
    /// Path to the agent crate directory (passed to `remi build --agent`).
    #[arg(long)]
    agent: PathBuf,

    /// Port to listen on.
    #[arg(long, default_value_t = 8080)]
    port: u16,

    /// Output directory where remi build places the compiled .wasm file.
    #[arg(long, default_value = "dist")]
    output: PathBuf,

    /// Path to remi-agentloop workspace root (auto-detected if omitted).
    #[arg(long)]
    remi_root: Option<PathBuf>,

    /// Disable file watching (build once, serve, no hot reload).
    #[arg(long)]
    no_watch: bool,

    /// Pass --debug to remi build (default: release).
    #[arg(long)]
    debug: bool,

    /// Path to the `remi` binary (default: look on PATH).
    #[arg(long, default_value = "remi")]
    remi_bin: String,
}

// ── Agent slot — shared mutable reference to the current agent ───────────────

/// Shared, hot-swappable WASM agent.
///
/// Write lock is held only during the brief swap after recompile.
/// Each request clones the inner `Arc<WasmAgentWithHttp>` under a short
/// read lock, then releases it immediately — no contention on I/O.
type AgentSlot = Arc<RwLock<Option<Arc<WasmAgentWithHttp>>>>;

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let agent_path = args
        .agent
        .canonicalize()
        .map_err(|e| format!("Cannot find agent crate {:?}: {e}", args.agent))?;

    let output = if args.output.is_absolute() {
        args.output.clone()
    } else {
        std::env::current_dir()?.join(&args.output)
    };
    std::fs::create_dir_all(&output)?;

    let crate_name = read_crate_name(&agent_path.join("Cargo.toml"));
    let wasm_path = output.join(format!("{crate_name}.wasm"));

    // ── Initial build + load ─────────────────────────────────────────
    println!("🔨  Building {} with `remi build`…", crate_name);
    compile_with_remi_build(
        &args.remi_bin,
        &agent_path,
        &output,
        args.debug,
        args.remi_root.as_deref(),
    )?;
    println!("✅  Build OK  →  {}", wasm_path.display());

    let agent = load_agent(&wasm_path).map_err(|e| e.to_string())?;
    let slot: AgentSlot = Arc::new(RwLock::new(Some(Arc::new(agent))));

    // ── File watcher ─────────────────────────────────────────────────
    if !args.no_watch {
        let slot_w = slot.clone();
        let agent_path_w = agent_path.clone();
        let output_w = output.clone();
        let wasm_path_w = wasm_path.clone();
        let remi_bin_w = args.remi_bin.clone();
        let remi_root_w = args.remi_root.clone();
        let debug = args.debug;

        tokio::spawn(watch_loop(
            slot_w,
            agent_path_w,
            output_w,
            wasm_path_w,
            remi_bin_w,
            remi_root_w,
            debug,
        ));
    }

    // ── HTTP SSE server ──────────────────────────────────────────────
    let addr: std::net::SocketAddr = ([0, 0, 0, 0], args.port).into();
    println!(
        "🚀  remi-dev-server  http://0.0.0.0:{}/chat  (agent: {})",
        args.port, crate_name
    );
    if !args.no_watch {
        println!("👀  Watching:  {}", agent_path.display());
    }

    let server = HttpSseServer::new(make_handler(slot)).bind(addr);
    server.serve().await?;

    Ok(())
}

// ── Server handler ────────────────────────────────────────────────────────────

fn make_handler(
    slot: AgentSlot,
) -> impl Fn(
    LoopInput,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = Result<ReceiverStream<ProtocolEvent>, ProtocolError>>
            + Send,
    >,
> + Send
       + Sync
       + 'static {
    move |req: LoopInput| {
        let slot = slot.clone();
        Box::pin(async move {
            // Briefly acquire read lock and clone the Arc — O(1), no I/O
            let agent: Arc<WasmAgentWithHttp> = {
                let guard = slot.read().unwrap();
                guard.as_ref().cloned().ok_or_else(|| ProtocolError {
                    code: "not_ready".into(),
                    message: "Agent not loaded yet — check build output".into(),
                })?
            };

            let (tx, rx) = tokio::sync::mpsc::channel::<ProtocolEvent>(32);

            // WasmAgentWithHttp::chat() returns a non-Send stream
            // (WIT wasmtime components are single-threaded), so we
            // run it on a dedicated OS thread with its own single-thread runtime.
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async move {
                    match agent.chat(remi_agentloop::types::ChatCtx::default(), req).await {
                        Ok(stream) => {
                            let mut stream = std::pin::pin!(stream);
                            while let Some(event) = stream.next().await {
                                if tx.send(event).await.is_err() {
                                    break; // client disconnected
                                }
                            }
                        }
                        Err(e) => {
                            let _ = tx
                                .send(ProtocolEvent::Error {
                                    message: e.to_string(),
                                    code: Some("agent_error".into()),
                                })
                                .await;
                        }
                    }
                });
            });

            Ok(ReceiverStream::new(rx))
        })
    }
}

// ── File watcher + hot-reload loop ────────────────────────────────────────────

async fn watch_loop(
    slot: AgentSlot,
    agent_path: PathBuf,
    output: PathBuf,
    wasm_path: PathBuf,
    remi_bin: String,
    remi_root: Option<PathBuf>,
    debug: bool,
) {
    let (change_tx, mut change_rx) = tokio::sync::mpsc::channel::<()>(4);

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let relevant = event.paths.iter().any(|p| {
                p.extension()
                    .map(|e| e == "rs" || e == "toml")
                    .unwrap_or(false)
            });
            if relevant {
                let _ = change_tx.blocking_send(());
            }
        }
    })
    .expect("Failed to create file watcher");

    watcher
        .watch(&agent_path, RecursiveMode::Recursive)
        .expect("Failed to watch agent directory");

    loop {
        if change_rx.recv().await.is_none() {
            break;
        }

        // Debounce: drain events that arrive within 500 ms
        let debounce_until = tokio::time::Instant::now() + Duration::from_millis(500);
        loop {
            match tokio::time::timeout_at(debounce_until, change_rx.recv()).await {
                Ok(Some(())) => continue,
                _ => break,
            }
        }

        println!("\n🔨  Source changed — rebuilding with `remi build`…");

        let agent_path2 = agent_path.clone();
        let output2 = output.clone();
        let wasm_path2 = wasm_path.clone();
        let remi_bin2 = remi_bin.clone();
        let remi_root2 = remi_root.clone();

        let result = tokio::task::spawn_blocking(move || {
            compile_with_remi_build(
                &remi_bin2,
                &agent_path2,
                &output2,
                debug,
                remi_root2.as_deref(),
            )?;
            load_agent(&wasm_path2).map_err(|e| e.to_string())
        })
        .await;

        match result {
            Ok(Ok(new_agent)) => {
                let mut guard = slot.write().unwrap();
                *guard = Some(Arc::new(new_agent));
                println!("✅  Hot-reloaded — serving new WASM agent");
            }
            Ok(Err(e)) => eprintln!("❌  Build/load failed: {e}"),
            Err(e) => eprintln!("❌  Task panicked: {e}"),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Invoke `remi build --agent <path> --targets wasip2 --output <output>`.
fn compile_with_remi_build(
    remi_bin: &str,
    agent_path: &Path,
    output: &Path,
    debug: bool,
    remi_root: Option<&Path>,
) -> Result<(), String> {
    let mut cmd = std::process::Command::new(remi_bin);
    cmd.arg("build")
        .arg("--agent")
        .arg(agent_path)
        .arg("--targets")
        .arg("wasip2")
        .arg("--output")
        .arg(output);

    if debug {
        cmd.arg("--release=false");
    }

    if let Some(root) = remi_root {
        cmd.arg("--remi-root").arg(root);
    }

    let status = cmd.status().map_err(|e| {
        format!("Cannot run `{remi_bin}`: {e}\nMake sure it is installed: cargo install --path remi-cli")
    })?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("`remi build` exited with {status}"))
    }
}

fn load_agent(wasm_path: &Path) -> Result<WasmAgentWithHttp, String> {
    WasmAgentWithHttp::from_file(wasm_path).map_err(|e| e.to_string())
}

/// Read `[package] name` from a Cargo.toml, replacing `-` → `_`.
fn read_crate_name(cargo_toml: &Path) -> String {
    let content = std::fs::read_to_string(cargo_toml).unwrap_or_default();
    let mut in_package = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[package]" {
            in_package = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_package = false;
        }
        if in_package {
            if let Some(rest) = trimmed.strip_prefix("name") {
                if let Some(rest) = rest.trim().strip_prefix('=') {
                    let name = rest.trim().trim_matches('"').trim_matches('\'');
                    return name.replace('-', "_");
                }
            }
        }
    }
    cargo_toml
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().replace('-', "_"))
        .unwrap_or_else(|| "agent".into())
}
