//! remi-cli build script.
//!
//! When the `dev` feature is active, builds the web UI from `ui/` using npm
//! so that `rust-embed` can embed the `dist-ui/` artefacts at compile time.
//!
//! Gracefully skips the step if:
//!   - the `dev` feature is not enabled
//!   - `node` / `npm` are not on PATH
//!   - the `dist-ui/` directory already exists and is newer than `ui/src/`
//!     (Cargo reruns via `rerun-if-changed` directives)

use std::path::PathBuf;
use std::process::Command;

fn main() {
    // Only run for the `dev` feature
    if std::env::var("CARGO_FEATURE_DEV").is_err() {
        return;
    }

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ui_dir = manifest_dir.join("ui");
    let dist_dir = manifest_dir.join("dist-ui");

    // Tell Cargo to rerun this script when UI sources change
    println!("cargo:rerun-if-changed=ui/src");
    println!("cargo:rerun-if-changed=ui/package.json");
    println!("cargo:rerun-if-changed=ui/vite.config.ts");
    println!("cargo:rerun-if-changed=ui/index.html");

    // Skip build if dist-ui already exists (fast path for CI)
    if std::env::var("REMI_SKIP_UI_BUILD").is_ok() {
        if dist_dir.exists() {
            return;
        } else {
            panic!(
                "REMI_SKIP_UI_BUILD is set but dist-ui/ does not exist. \
                 Run `npm run build` in remi-cli/ui/ first."
            );
        }
    }

    // Check for npm
    let npm_available = Command::new("npm")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !npm_available {
        if dist_dir.exists() {
            // Warn but don't fail — use the existing dist-ui/
            println!(
                "cargo:warning=npm not found; using existing dist-ui/. \
                 Run `npm run build` in remi-cli/ui/ to rebuild the web UI."
            );
            return;
        } else {
            panic!(
                "npm not found on PATH and dist-ui/ does not exist. \
                 Install Node.js or build the UI manually: \
                 `cd remi-cli/ui && npm install && npm run build`"
            );
        }
    }

    // npm install (offline-preferred — fast when node_modules already exist)
    let install_status = Command::new("npm")
        .args(["install", "--prefer-offline", "--no-audit", "--no-fund"])
        .current_dir(&ui_dir)
        .status()
        .expect("failed to run `npm install`");

    if !install_status.success() {
        panic!("`npm install` failed in remi-cli/ui/");
    }

    // npm run build
    let build_status = Command::new("npm")
        .args(["run", "build"])
        .current_dir(&ui_dir)
        .status()
        .expect("failed to run `npm run build`");

    if !build_status.success() {
        panic!("`npm run build` failed in remi-cli/ui/");
    }
}
