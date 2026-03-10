//! `remi dev` — Hot-reloading WASM agent dev server with built-in web UI.
//!
//! Routes:
//!   POST /chat     — Agent SSE stream
//!   GET  /status   — Build status SSE stream (hot-reload notifications)
//!   GET  /         — Built-in React web UI
//!   GET  /assets/* — Bundled JS / CSS assets

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use axum::extract::State;
use axum::response::sse::{Event, Sse};
use axum::routing::get;
use clap::Parser;
use futures::{Stream, StreamExt};
use notify::{RecursiveMode, Watcher};
use serde_json::json;
use tokio::sync::broadcast;
use tokio_stream::wrappers::{BroadcastStream, ReceiverStream};
use tower_http::cors::CorsLayer;

use remi_agentloop::agent::Agent;
use remi_agentloop::protocol::{ProtocolError, ProtocolEvent};
use remi_agentloop::transport::HttpSseServer;
use remi_agentloop::types::LoopInput;
use remi_agentloop_wasm::WasmAgentWithHttp;

use crate::ui::ui_router;

// ── CLI args ──────────────────────────────────────────────────────────────────

#[derive(Parser)]
pub struct DevArgs {
    /// Path to the agent crate directory.
    #[arg(long)]
    pub agent: PathBuf,

    /// Port to listen on.
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// Output directory for compiled .wasm.
    #[arg(long, default_value = "dist")]
    pub output: PathBuf,

    /// Path to remi-agentloop workspace root.
    #[arg(long)]
    pub remi_root: Option<PathBuf>,

    /// Disable file watching.
    #[arg(long)]
    pub no_watch: bool,

    /// Use debug (non-release) build.
    #[arg(long)]
    pub debug: bool,

    /// Skip the initial `remi build` step and load the WASM directly from
    /// `--output/<crate_name>.wasm`. Useful when the agent is already compiled.
    #[arg(long)]
    pub skip_build: bool,
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(args: DevArgs) {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
        .block_on(run_async(args))
        .unwrap_or_else(|e| {
            eprintln!("remi dev: error: {e}");
            std::process::exit(1);
        });
}

// ── Build status event ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum BuildStatusEvent {
    BuildStart,
    BuildOk { crate_name: String },
    BuildError { message: String },
    AgentReloaded { crate_name: String },
    Ping,
}

impl BuildStatusEvent {
    fn sse_event_name(&self) -> &'static str {
        match self {
            Self::BuildStart => "build_start",
            Self::BuildOk { .. } => "build_ok",
            Self::BuildError { .. } => "build_error",
            Self::AgentReloaded { .. } => "agent_reloaded",
            Self::Ping => "ping",
        }
    }

    fn sse_data(&self) -> String {
        match self {
            Self::BuildStart => json!({"type":"build_start"}).to_string(),
            Self::BuildOk { crate_name } => {
                json!({"type":"build_ok","crate_name":crate_name}).to_string()
            }
            Self::BuildError { message } => {
                json!({"type":"build_error","message":message}).to_string()
            }
            Self::AgentReloaded { crate_name } => {
                json!({"type":"agent_reloaded","crate_name":crate_name}).to_string()
            }
            Self::Ping => json!({"type":"ping"}).to_string(),
        }
    }
}

type BuildStatusTx = Arc<broadcast::Sender<BuildStatusEvent>>;

// ── Async core ────────────────────────────────────────────────────────────────

type AgentSlot = Arc<RwLock<Option<Arc<WasmAgentWithHttp>>>>;

/// LLM credentials read from environment variables at startup.
/// Mirrors the env vars used by `remi-tui`.
#[derive(Debug, Clone, Default)]
struct EnvCreds {
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
}

impl EnvCreds {
    fn from_env() -> Self {
        Self {
            api_key: std::env::var("OPENAI_API_KEY")
                .or_else(|_| std::env::var("REMI_API_KEY"))
                .ok()
                .filter(|s| !s.is_empty()),
            base_url: std::env::var("REMI_BASE_URL")
                .or_else(|_| std::env::var("OPENAI_BASE_URL"))
                .ok()
                .filter(|s| !s.is_empty()),
            model: std::env::var("REMI_MODEL").ok().filter(|s| !s.is_empty()),
        }
    }

    /// Inject into a LoopInput's metadata — only fills fields not already set
    /// by the caller (e.g. from the web UI settings).
    fn inject(&self, req: LoopInput) -> LoopInput {
        match req {
            LoopInput::Start {
                content,
                history,
                extra_tools,
                model,
                temperature,
                max_tokens,
                metadata,
            } => {
                let mut meta: serde_json::Map<String, serde_json::Value> = match &metadata {
                    Some(serde_json::Value::Object(m)) => m.clone(),
                    _ => serde_json::Map::new(),
                };
                if !meta.contains_key("api_key") {
                    if let Some(k) = &self.api_key {
                        meta.insert("api_key".into(), serde_json::Value::String(k.clone()));
                    }
                }
                if !meta.contains_key("base_url") {
                    if let Some(u) = &self.base_url {
                        meta.insert("base_url".into(), serde_json::Value::String(u.clone()));
                    }
                }
                if !meta.contains_key("model") {
                    if let Some(m) = &self.model {
                        meta.insert("model".into(), serde_json::Value::String(m.clone()));
                    }
                }
                let new_meta = if meta.is_empty() {
                    metadata
                } else {
                    Some(serde_json::Value::Object(meta))
                };
                LoopInput::Start {
                    content,
                    history,
                    extra_tools,
                    model,
                    temperature,
                    max_tokens,
                    metadata: new_meta,
                }
            }
            other => other,
        }
    }
}

async fn run_async(args: DevArgs) -> Result<(), Box<dyn std::error::Error>> {
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

    let remi_root: Option<PathBuf> = args
        .remi_root
        .map(|p| p.canonicalize().expect("cannot resolve --remi-root"));

    let crate_name = read_crate_name(&agent_path.join("Cargo.toml"));
    let wasm_path = output.join(format!("{crate_name}.wasm"));

    if args.skip_build {
        println!(
            "⏭️   Skipping build, loading existing WASM from {}",
            wasm_path.display()
        );
        if !wasm_path.exists() {
            return Err(format!(
                "WASM file not found: {}\n  Run without --skip-build to compile first.",
                wasm_path.display()
            )
            .into());
        }
    } else {
        println!("🔨  Building {} with `remi build`…", crate_name);
        compile_agent(&agent_path, &output, args.debug, remi_root.as_deref())?;
        println!("✅  Build OK  →  {}", wasm_path.display());
    }

    let agent = load_agent(&wasm_path)?;
    let slot: AgentSlot = Arc::new(RwLock::new(Some(Arc::new(agent))));

    // Read LLM credentials from env vars (same vars as remi-tui)
    let creds = EnvCreds::from_env();
    if creds.api_key.is_some() {
        let model_str = creds.model.as_deref().unwrap_or("(not set)");
        let url_str = creds
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1");
        println!("🔑  Credentials: model={model_str}  base_url={url_str}");
    } else {
        println!("⚠️   No API key found — set OPENAI_API_KEY or configure in the web UI.");
    }
    let creds = Arc::new(creds);

    // Build-status broadcast channel
    let (status_tx_raw, _) = broadcast::channel::<BuildStatusEvent>(32);
    let status_tx: BuildStatusTx = Arc::new(status_tx_raw);

    if !args.no_watch {
        let slot_w = slot.clone();
        let tx_w = status_tx.clone();
        let agent_path_w = agent_path.clone();
        let output_w = output.clone();
        let wasm_path_w = wasm_path.clone();
        let remi_root_w = remi_root.clone();
        let debug = args.debug;
        let cn = crate_name.clone();

        tokio::spawn(watch_loop(
            slot_w,
            tx_w,
            agent_path_w,
            output_w,
            wasm_path_w,
            remi_root_w,
            debug,
            cn,
        ));
    }

    // /chat
    let chat_router = HttpSseServer::new(make_handler(slot, creds)).router();

    // /status
    let status_router = axum::Router::new()
        .route("/status", get(status_sse_handler))
        .with_state(status_tx.clone());

    let app = chat_router
        .merge(status_router)
        .merge(ui_router())
        .layer(CorsLayer::permissive());

    let addr: std::net::SocketAddr = ([0, 0, 0, 0], args.port).into();
    println!(
        "🚀  remi dev   → http://localhost:{}/chat  (agent: {})",
        args.port, crate_name
    );
    println!("🌐  Web UI     → http://localhost:{}/", args.port);
    if !args.no_watch {
        println!("👀  Watching:  {}", agent_path.display());
    }

    // Keep-alive pings every 30 s
    let tx_ping = status_tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let _ = tx_ping.send(BuildStatusEvent::Ping);
        }
    });

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

// ── /status SSE handler ───────────────────────────────────────────────────────

async fn status_sse_handler(
    State(tx): State<BuildStatusTx>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| {
        let result = match msg {
            Ok(event) => Some(Ok(Event::default()
                .event(event.sse_event_name())
                .data(event.sse_data()))),
            Err(_) => None,
        };
        async move { result }
    });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(20))
            .text("keep-alive"),
    )
}

// ── Chat handler ──────────────────────────────────────────────────────────────

fn make_handler(
    slot: AgentSlot,
    creds: Arc<EnvCreds>,
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
        let req = creds.inject(req);
        Box::pin(async move {
            let agent: Arc<WasmAgentWithHttp> = {
                let guard = slot.read().unwrap();
                guard.as_ref().cloned().ok_or_else(|| ProtocolError {
                    code: "not_ready".into(),
                    message: "Agent not loaded yet — check build output".into(),
                })?
            };

            let (tx, rx) = tokio::sync::mpsc::channel::<ProtocolEvent>(32);

            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async move {
                    match agent.chat(req).await {
                        Ok(stream) => {
                            let mut stream = std::pin::pin!(stream);
                            while let Some(event) = stream.next().await {
                                if tx.send(event).await.is_err() {
                                    break;
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

#[allow(clippy::too_many_arguments)]
async fn watch_loop(
    slot: AgentSlot,
    status_tx: BuildStatusTx,
    agent_path: PathBuf,
    output: PathBuf,
    wasm_path: PathBuf,
    remi_root: Option<PathBuf>,
    debug: bool,
    crate_name: String,
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

        // Debounce
        let debounce_until = tokio::time::Instant::now() + Duration::from_millis(500);
        loop {
            match tokio::time::timeout_at(debounce_until, change_rx.recv()).await {
                Ok(Some(())) => continue,
                _ => break,
            }
        }

        println!("\n🔨  Source changed — rebuilding…");
        let _ = status_tx.send(BuildStatusEvent::BuildStart);

        let agent_path2 = agent_path.clone();
        let output2 = output.clone();
        let wasm_path2 = wasm_path.clone();
        let remi_root2 = remi_root.clone();
        let cn2 = crate_name.clone();

        let result = tokio::task::spawn_blocking(move || {
            compile_agent(&agent_path2, &output2, debug, remi_root2.as_deref())?;
            load_agent(&wasm_path2).map(|a| (a, cn2))
        })
        .await;

        match result {
            Ok(Ok((new_agent, cn))) => {
                {
                    let mut guard = slot.write().unwrap();
                    *guard = Some(Arc::new(new_agent));
                }
                println!("✅  Hot-reloaded — serving new WASM agent");
                let _ = status_tx.send(BuildStatusEvent::BuildOk {
                    crate_name: cn.clone(),
                });
                let _ = status_tx.send(BuildStatusEvent::AgentReloaded { crate_name: cn });
            }
            Ok(Err(e)) => {
                let msg = e.to_string();
                eprintln!("❌  Build/load failed: {msg}");
                let _ = status_tx.send(BuildStatusEvent::BuildError { message: msg });
            }
            Err(e) => {
                let msg = e.to_string();
                eprintln!("❌  Task panicked: {msg}");
                let _ = status_tx.send(BuildStatusEvent::BuildError { message: msg });
            }
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn compile_agent(
    agent_path: &Path,
    output: &Path,
    debug: bool,
    remi_root: Option<&Path>,
) -> Result<(), String> {
    let remi_bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("remi"));

    let mut cmd = std::process::Command::new(&remi_bin);
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

    let status = cmd
        .status()
        .map_err(|e| format!("Cannot run `{}`: {e}", remi_bin.display()))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!("`remi build` exited with {status}"))
    }
}

fn load_agent(wasm_path: &Path) -> Result<WasmAgentWithHttp, String> {
    WasmAgentWithHttp::from_file(wasm_path).map_err(|e| e.to_string())
}

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
