//! Embedded web UI assets served from `dist-ui/` at compile time via
//! [`rust_embed`]. Provides a single `axum::Router` that serves:
//!
//!   `GET /`           → `index.html`
//!   `GET /assets/*`   → hashed JS / CSS bundles

use std::borrow::Cow;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use rust_embed::RustEmbed;

/// Embedded production build of `remi-agentloop-cli/ui/`.
/// Built by `remi-agentloop-cli/build.rs` via `npm run build`.
#[derive(RustEmbed)]
#[folder = "dist-ui/"]
struct UiAssets;

// ── Handlers ─────────────────────────────────────────────────────────────────

async fn serve_index() -> impl IntoResponse {
    serve_asset("index.html".to_string()).await
}

async fn serve_asset(path: String) -> impl IntoResponse {
    let path = path.trim_start_matches('/');

    match UiAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let data: Cow<'static, [u8]> = content.data;
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime.as_ref())
                // Cache hashed assets indefinitely; main index.html no-cache
                .header(
                    header::CACHE_CONTROL,
                    if path == "index.html" {
                        "no-cache, no-store"
                    } else {
                        "public, max-age=31536000, immutable"
                    },
                )
                .body(Body::from(data.into_owned()))
                .unwrap()
                .into_response()
        }
        None => {
            // Fallback to index.html for SPA client-side routing
            if let Some(index) = UiAssets::get("index.html") {
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .header(header::CACHE_CONTROL, "no-cache, no-store")
                    .body(Body::from(index.data.into_owned()))
                    .unwrap()
                    .into_response()
            } else {
                (StatusCode::NOT_FOUND, "UI build not found").into_response()
            }
        }
    }
}

async fn serve_asset_path(Path(path): Path<String>) -> impl IntoResponse {
    // Route is /assets/*path, so `path` = "index-xxx.js"
    // but rust-embed keys the file as "assets/index-xxx.js"
    let full = format!("assets/{}", path.trim_start_matches('/'));
    serve_asset(full).await
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Returns an axum `Router` that serves the embedded web UI.
///
/// Routes:
/// - `GET /`          → `index.html`
/// - `GET /assets/*path`  → bundled JS / CSS assets (catch-all)
pub fn ui_router() -> axum::Router {
    axum::Router::new()
        .route("/", get(serve_index))
        .route("/assets/*path", get(serve_asset_path))
        // SPA catch-all: any unknown GET route falls back to index.html
        .fallback(get(serve_index))
}
