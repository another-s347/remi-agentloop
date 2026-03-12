//! remi — Remi agent toolchain.
//!
//! Subcommands:
//!   remi build  — compile an agent crate into WASM targets (wasip2, web, or both)
//!   remi dev    — hot-reloading WASM agent dev server (requires `--features dev`)
//!
//! Usage:
//! ```sh
//! remi build --agent ./my-agent                     # both targets
//! remi build --agent ./my-agent --targets wasip2    # wasip2 only
//! remi build --agent ./my-agent --targets web       # browser only
//! remi build --agent ./my-agent --output ./dist     # custom output dir
//! remi build --agent ./my-agent --precompile-targets aarch64-linux-android
//!
//! remi dev --agent ./my-agent --port 8080           # hot-reload dev server
//! ```

use clap::{Parser, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(feature = "dev")]
mod dev;
mod templates;
#[cfg(feature = "dev")]
mod ui;

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "remi", version, about = "Remi agent toolchain")]
enum Cli {
    /// Build an agent crate into WASM targets (wasip2, web, or both).
    Build(BuildArgs),

    /// Start a hot-reloading WASM agent dev server (HTTP SSE).
    ///
    /// Compiles the agent with `remi build` on startup and re-compiles on every
    /// source change, then hot-swaps the WASM module without restarting.
    ///
    /// Only available when the `dev` feature is enabled:
    ///   cargo install --path remi-cli --features dev
    #[cfg(feature = "dev")]
    Dev(dev::DevArgs),
}

#[derive(Parser)]
struct BuildArgs {
    /// Path to the agent crate (must have build_agent<T>() function).
    #[arg(long)]
    agent: PathBuf,

    /// Which WASM targets to build.
    #[arg(long, value_delimiter = ',', default_value = "wasip2,web")]
    targets: Vec<Target>,

    /// Output directory for build artifacts.
    #[arg(long, default_value = "dist")]
    output: PathBuf,

    /// Function name to call in the agent crate.
    #[arg(long, default_value = "build_agent")]
    entry: String,

    /// Release mode (default: true).
    #[arg(long, default_value_t = true)]
    release: bool,

    /// AOT-precompile the wasip2 output for these host triples (comma-separated).
    ///
    /// Requires building with `--features precompile` and that `wasip2` is in
    /// `--targets`. Each triple produces `<output>/<agent>.{triple}.cwasm`.
    ///
    /// Example: --precompile-targets aarch64-linux-android
    #[arg(long, value_delimiter = ',')]
    precompile_targets: Vec<String>,
}

#[derive(Clone, Debug, ValueEnum, PartialEq)]
enum Target {
    Wasip2,
    Web,
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    match Cli::parse() {
        Cli::Build(args) => run_build(args),
        #[cfg(feature = "dev")]
        Cli::Dev(args) => dev::run(args),
    }
}

fn run_build(args: BuildArgs) {
    let agent_path = std::fs::canonicalize(&args.agent).unwrap_or_else(|e| {
        eprintln!("Error: cannot find agent crate at {:?}: {e}", args.agent);
        std::process::exit(1);
    });

    // Read agent crate name from Cargo.toml
    let agent_name = read_crate_name(&agent_path);
    println!("Agent crate: {agent_name} ({agent_path:?})");

    // Create output dir
    let output = if args.output.is_absolute() {
        args.output.clone()
    } else {
        std::env::current_dir().unwrap().join(&args.output)
    };
    std::fs::create_dir_all(&output).expect("cannot create output dir");

    let mut ok = true;

    for target in &args.targets {
        let success = match target {
            Target::Wasip2 => build_wasip2(
                &agent_path,
                &agent_name,
                &args.entry,
                &output,
                args.release,
            ),
            Target::Web => build_web(
                &agent_path,
                &agent_name,
                &args.entry,
                &output,
                args.release,
            ),
        };
        if !success {
            ok = false;
        }
    }

    if ok {
        println!("\n✅ All targets built successfully. Output: {output:?}");
    } else {
        eprintln!("\n❌ Some targets failed.");
        std::process::exit(1);
    }

    // Optional AOT precompile step (requires --features precompile).
    #[cfg(feature = "precompile")]
    if !args.precompile_targets.is_empty() {
        if !run_precompile_step(&args.precompile_targets, &agent_name, &output) {
            std::process::exit(1);
        }
    }

    #[cfg(not(feature = "precompile"))]
    if !args.precompile_targets.is_empty() {
        eprintln!(
            "Warning: --precompile-targets specified but remi-cli was built without the \
             `precompile` feature. Rebuild with `--features precompile`."
        );
    }
}

// ── AOT precompile ───────────────────────────────────────────────────────────

/// Cross-compile `<agent>.wasm` into one `.cwasm` blob per target triple.
///
/// Only compiled when the `precompile` feature is enabled.
#[cfg(feature = "precompile")]
fn run_precompile_step(triples: &[String], agent_name: &str, output: &Path) -> bool {
    let wasm_path = output.join(format!("{agent_name}.wasm"));
    if !wasm_path.exists() {
        eprintln!(
            "\n❌ Precompile: {wasm_path:?} not found. \
             Make sure `--targets wasip2` is included."
        );
        return false;
    }
    let wasm_bytes = std::fs::read(&wasm_path).unwrap_or_else(|e| {
        eprintln!("❌ Cannot read {wasm_path:?}: {e}");
        std::process::exit(1);
    });

    println!("\n── AOT precompile ──────────────────────────────────────");
    let mut ok = true;
    for triple in triples {
        // Normalise triple for use in a filename: replace non-alphanumeric chars with '_'
        let normalized: String = triple
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '-' })
            .collect();
        let out_path = output.join(format!("{agent_name}.{normalized}.cwasm"));
        print!("  {triple} → {} … ", out_path.display());
        match remi_agentloop_wasm::WasmAgent::precompile_for_target(&wasm_bytes, triple) {
            Ok(cwasm) => {
                std::fs::write(&out_path, &cwasm).unwrap_or_else(|e| {
                    eprintln!("❌ Cannot write {out_path:?}: {e}");
                    std::process::exit(1);
                });
                println!("✅ ({:.0} KB)", cwasm.len() as f64 / 1024.0);
            }
            Err(e) => {
                println!("❌");
                eprintln!("    Error: {}: {}", e.code, e.message);
                ok = false;
            }
        }
    }
    if ok {
        println!("\n✅ Precompile complete.");
    } else {
        eprintln!("\n❌ Precompile failed for one or more targets.");
    }
    ok
}

// ── Build: wasip2 ────────────────────────────────────────────────────────────

fn build_wasip2(
    agent_path: &Path,
    agent_name: &str,
    entry_fn: &str,
    output: &Path,
    release: bool,
) -> bool {
    println!("\n── Building wasip2 ──────────────────────────────────────");

    // Check target is installed
    if !check_target_installed("wasm32-wasip2") {
        eprintln!("Error: target wasm32-wasip2 not installed. Run:");
        eprintln!("  rustup target add wasm32-wasip2");
        return false;
    }

    let tmp = tempfile::Builder::new()
        .prefix("remi-wasip2-")
        .tempdir()
        .expect("cannot create temp dir");
    let crate_dir = tmp.path();

    // Generate Cargo.toml
    let cargo_toml = templates::wasip2_cargo_toml(
        agent_name,
        &agent_path.display().to_string(),
    );
    std::fs::write(crate_dir.join("Cargo.toml"), cargo_toml).unwrap();

    // Generate src/lib.rs
    let src_dir = crate_dir.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let lib_rs = templates::wasip2_lib_rs(agent_name, entry_fn);
    std::fs::write(src_dir.join("lib.rs"), lib_rs).unwrap();

    // Build
    let mut cmd = Command::new("cargo");
    cmd.arg("build")
        .arg("--target")
        .arg("wasm32-wasip2")
        .current_dir(crate_dir);
    if release {
        cmd.arg("--release");
    }

    println!(
        "  Running: cargo build --target wasm32-wasip2 {}",
        if release { "--release" } else { "" }
    );
    let status = cmd.status();
    match status {
        Ok(s) if s.success() => {
            // Copy output
            let profile = if release { "release" } else { "debug" };
            let wasm_file = crate_dir
                .join("target")
                .join("wasm32-wasip2")
                .join(profile)
                .join(format!(
                    "{}_wasip2_entry.wasm",
                    agent_name.replace('-', "_")
                ));
            let dest = output.join(format!("{agent_name}.wasm"));
            if wasm_file.exists() {
                std::fs::copy(&wasm_file, &dest).unwrap();
                let size = std::fs::metadata(&dest).unwrap().len();
                println!(
                    "  ✅ wasip2: {} ({:.0} KB)",
                    dest.display(),
                    size as f64 / 1024.0
                );
                true
            } else {
                eprintln!("  ❌ Expected output not found: {}", wasm_file.display());
                // Try to find it
                find_wasm_in_dir(&crate_dir.join("target").join("wasm32-wasip2").join(profile));
                false
            }
        }
        Ok(s) => {
            eprintln!("  ❌ cargo build failed with: {s}");
            false
        }
        Err(e) => {
            eprintln!("  ❌ cannot run cargo: {e}");
            false
        }
    }
}

// ── Build: web (browser) ─────────────────────────────────────────────────────

fn build_web(
    agent_path: &Path,
    agent_name: &str,
    entry_fn: &str,
    output: &Path,
    release: bool,
) -> bool {
    println!("\n── Building web (browser) ──────────────────────────────");

    // Check wasm-pack is installed
    if Command::new("wasm-pack").arg("--version").output().is_err() {
        eprintln!("Error: wasm-pack not installed. Run:");
        eprintln!("  cargo install wasm-pack");
        return false;
    }

    let tmp = tempfile::Builder::new()
        .prefix("remi-web-")
        .tempdir()
        .expect("cannot create temp dir");
    let crate_dir = tmp.path();

    // Generate Cargo.toml
    let cargo_toml = templates::web_cargo_toml(
        agent_name,
        &agent_path.display().to_string(),
    );
    std::fs::write(crate_dir.join("Cargo.toml"), cargo_toml).unwrap();

    // Generate src/lib.rs
    let src_dir = crate_dir.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let lib_rs = templates::web_lib_rs(agent_name, entry_fn);
    std::fs::write(src_dir.join("lib.rs"), lib_rs).unwrap();

    // Build with wasm-pack
    let mut cmd = Command::new("wasm-pack");
    cmd.arg("build")
        .arg("--target")
        .arg("web")
        .current_dir(crate_dir);
    if release {
        cmd.arg("--release");
    }

    println!(
        "  Running: wasm-pack build --target web {}",
        if release { "--release" } else { "" }
    );
    let status = cmd.status();
    match status {
        Ok(s) if s.success() => {
            // Copy pkg/ directory to output
            let pkg_dir = crate_dir.join("pkg");
            let dest_dir = output.join(format!("{agent_name}-web"));
            if pkg_dir.exists() {
                copy_dir_recursive(&pkg_dir, &dest_dir);
                println!("  ✅ web: {}/", dest_dir.display());
                true
            } else {
                eprintln!("  ❌ Expected pkg/ directory not found");
                false
            }
        }
        Ok(s) => {
            eprintln!("  ❌ wasm-pack build failed with: {s}");
            false
        }
        Err(e) => {
            eprintln!("  ❌ cannot run wasm-pack: {e}");
            false
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn read_crate_name(crate_path: &Path) -> String {
    let cargo_toml = crate_path.join("Cargo.toml");
    let content = std::fs::read_to_string(&cargo_toml).unwrap_or_else(|e| {
        eprintln!("Error: cannot read {:?}: {e}", cargo_toml);
        std::process::exit(1);
    });
    let doc = content
        .parse::<toml_edit::DocumentMut>()
        .unwrap_or_else(|e| {
            eprintln!("Error: cannot parse {:?}: {e}", cargo_toml);
            std::process::exit(1);
        });
    doc["package"]["name"]
        .as_str()
        .unwrap_or_else(|| {
            eprintln!("Error: no [package] name in {:?}", cargo_toml);
            std::process::exit(1);
        })
        .to_string()
}

fn check_target_installed(target: &str) -> bool {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).contains(target),
        Err(_) => false,
    }
}

fn find_wasm_in_dir(dir: &Path) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().map(|e| e == "wasm").unwrap_or(false) {
                eprintln!("  Found: {}", p.display());
            }
        }
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let ty = entry.file_type().unwrap();
        let dest = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}
