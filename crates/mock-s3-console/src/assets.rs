//! The console's static assets.
//!
//! Embedded from `web/dist` in a release build, so the binary is self-contained and the
//! container needs nothing beside it. In a debug build `rust-embed` reads the same files
//! from disk instead, so `pnpm dev` and `cargo run` can be used together without
//! rebuilding the server to see a change.
//!
//! When `web/dist` holds no `index.html`: a fresh checkout where nobody has run
//! `pnpm build`: this serves a page saying so rather than a 404, because a blank
//! console with no explanation is the least useful failure available.
//!
//! The directory itself is tracked, empty, with a `.gitkeep`. `rust-embed` resolves the
//! folder when the derive expands, so a missing directory is a *compile* error, and
//! without the placeholder the whole workspace would fail to build for anyone who had
//! not built the console first.

use axum::response::{Html, IntoResponse, Response};
use http::{StatusCode, header};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Assets;

const INDEX: &str = "index.html";

/// Serve one asset by path, or the console's `index.html` for anything else.
///
/// Unknown paths fall back to `index.html` on purpose: the console routes in the
/// browser, so `/buckets/my-bucket` is a page the server knows nothing about and must
/// still answer with the app rather than a 404.
pub fn serve(path: &str) -> Response {
    let trimmed = path.trim_start_matches('/');

    if !trimmed.is_empty()
        && let Some(file) = Assets::get(trimmed)
    {
        let mime = file.metadata.mimetype().to_string();

        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, mime)],
            file.data.into_owned(),
        )
            .into_response();
    }

    index()
}

/// The console itself.
pub fn index() -> Response {
    match Assets::get(INDEX) {
        Some(file) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            file.data.into_owned(),
        )
            .into_response(),
        None => Html(NOT_BUILT).into_response(),
    }
}

/// Shown when `web/dist` is absent. The admin API still works, so this says how to reach
/// it and how to build the console.
const NOT_BUILT: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>mock-s3</title>
<style>
  :root { color-scheme: light dark; --fg: #16161a; --bg: #fbfbfd; --muted: #6b6b76; --line: #e4e4ea; }
  @media (prefers-color-scheme: dark) {
    :root { --fg: #f2f2f5; --bg: #121216; --muted: #9a9aa6; --line: #2a2a32; }
  }
  * { box-sizing: border-box; }
  body { margin: 0; background: var(--bg); color: var(--fg);
         font: 15px/1.6 ui-sans-serif, system-ui, -apple-system, sans-serif; }
  main { max-width: 42rem; margin: 0 auto; padding: 4rem 16px; }
  h1 { font-size: 1.5rem; margin: 0 0 .25rem; letter-spacing: -.01em; }
  p.lede { color: var(--muted); margin: 0 0 2.5rem; }
  h2 { font-size: .8rem; letter-spacing: .02em; color: var(--muted);
       margin: 2.5rem 0 .75rem; font-weight: 600; }
  table { width: 100%; border-collapse: collapse; }
  td { padding: .5rem 0; border-bottom: 1px solid var(--line); vertical-align: top; }
  td:first-child { width: 13rem; }
  code { font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; }
</style>
</head>
<body>
<main>
  <h1>mock-s3</h1>
  <p class="lede">The S3 API is live. The console has not been built in this checkout.</p>

  <h2>Build it</h2>
  <table>
    <tr><td><code>pnpm --dir web install</code></td><td>Once</td></tr>
    <tr><td><code>pnpm --dir web build</code></td><td>Writes <code>web/dist</code></td></tr>
    <tr><td><code>cargo xtask dist</code></td><td>Both, then the release binary</td></tr>
  </table>

  <h2>Admin API, which works either way</h2>
  <table>
    <tr><td><code>GET /_admin/health</code></td><td>Region, backend, bucket count</td></tr>
    <tr><td><code>GET /_admin/buckets</code></td><td>Buckets with counts and sizes</td></tr>
    <tr><td><code>GET /_admin/requests</code></td><td>What clients have been sending</td></tr>
    <tr><td><code>POST /_admin/reset</code></td><td>Drop every bucket</td></tr>
  </table>
</main>
</body>
</html>
"#;
