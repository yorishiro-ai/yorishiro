//! Serves the Yorishiro setup/login/admin-dashboard SPA (this crate's own `web/`), compiled
//! into the binary at build time via `rust-embed`. This is what lets `yorishiro-server` (and,
//! via that crate's `build_app`, `yorishiro-hosted-server` too -- see that repo) serve a working
//! web UI without a deployment needing to separately fetch and place a `web/` directory
//! alongside the binary; the release tarball and Docker image both only ever shipped the binary
//! itself.
//!
//! An operator actively iterating on `web/`'s contents can still point at a real directory on
//! disk instead of the compiled-in copy (`YSR_WEB_DIR` in yorishiro-server,
//! `YORISHIRO_HOSTED_WEB_DIR` in yorishiro-hosted-server) -- see [`fallback_service`]. That
//! directory is read fresh on every request, so edits show up without a rebuild.

use std::path::{Component, Path, PathBuf};

use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/"]
struct Assets;

/// Maps a request path to the asset path it should serve: `/` (and the empty path) map to
/// `index.html`, same as `ServeDir`'s default `index_file` behavior; everything else is used
/// as-is, relative to `web/`.
fn asset_path(uri_path: &str) -> &str {
    match uri_path.trim_start_matches('/') {
        "" => "index.html",
        other => other,
    }
}

fn respond(path: &str, bytes: Vec<u8>) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .body(Body::from(bytes))
        // A well-formed content-type header value and a non-streaming body never fail to
        // build a response.
        .expect("building a static-asset response is infallible")
}

fn has_file_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some()
}

fn serve_embedded(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => respond(path, file.data.into_owned()),
        None if has_file_extension(path) => StatusCode::NOT_FOUND.into_response(),
        None => match Assets::get("index.html") {
            Some(file) => respond("index.html", file.data.into_owned()),
            None => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

/// Rejects any path containing a segment other than a plain filename/directory component --
/// `..`, a bare `.`, an absolute-path root, or (on Windows) a drive prefix -- so
/// `serve_from_disk` can never be made to read a file outside `dir` via a crafted request path
/// such as `/../../etc/passwd`.
fn is_safe_relative_path(path: &str) -> bool {
    Path::new(path)
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
}

async fn serve_from_disk(dir: &Path, path: &str) -> Response {
    if !is_safe_relative_path(path) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match tokio::fs::read(dir.join(path)).await {
        Ok(bytes) => respond(path, bytes),
        Err(_) if has_file_extension(path) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => match tokio::fs::read(dir.join("index.html")).await {
            Ok(bytes) => respond("index.html", bytes),
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        },
    }
}

/// A fallback service (for `Router::fallback_service`) that serves the SPA's static files.
/// `override_dir`, when `Some`, serves from that directory on disk instead of the assets
/// compiled into the binary -- see the module docs.
pub fn fallback_service(override_dir: Option<String>) -> MethodRouter {
    let override_dir = override_dir.map(PathBuf::from);
    get(move |uri: Uri| {
        let override_dir = override_dir.clone();
        async move {
            let path = asset_path(uri.path()).to_string();
            match override_dir {
                Some(dir) => serve_from_disk(&dir, &path).await,
                None => serve_embedded(&path),
            }
        }
    })
}
