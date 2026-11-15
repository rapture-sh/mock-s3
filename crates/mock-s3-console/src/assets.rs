//! The console's static assets.
//!
//! The React console is not built yet. Until `web/` exists, this serves a page that says
//! so and points at the admin API, which is more useful than a 404. When the frontend
//! lands this module switches to `rust-embed` over `web/dist`: reading from disk in
//! debug builds, embedded in the binary in release builds.

use axum::response::{Html, IntoResponse, Response};

const PLACEHOLDER: &str = r#"<!doctype html>
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
  h2 { font-size: .8rem; text-transform: uppercase; letter-spacing: .08em;
       color: var(--muted); margin: 2.5rem 0 .75rem; font-weight: 600; }
  table { width: 100%; border-collapse: collapse; }
  td { padding: .5rem 0; border-bottom: 1px solid var(--line); vertical-align: top; }
  td:first-child { width: 11rem; }
  code { font: 13px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace; }
</style>
</head>
<body>
<main>
  <h1>mock-s3</h1>
  <p class="lede">The S3 API is live. This console is still being built.</p>

  <h2>Admin API</h2>
  <table>
    <tr><td><code>GET /_admin/health</code></td><td>Liveness, region and backend</td></tr>
    <tr><td><code>GET /_admin/buckets</code></td><td>Buckets with object counts and sizes</td></tr>
    <tr><td><code>POST /_admin/reset</code></td><td>Drop every bucket</td></tr>
    <tr><td><code>POST /_admin/reset/{bucket}</code></td><td>Empty one bucket, keep it</td></tr>
  </table>

  <h2>Pointing a client at it</h2>
  <table>
    <tr><td><code>Endpoint</code></td><td>The S3 port, not this one</td></tr>
    <tr><td><code>Addressing</code></td><td>Path-style &mdash; set <code>forcePathStyle</code></td></tr>
    <tr><td><code>Credentials</code></td><td>Anything; they are parsed, never verified</td></tr>
  </table>
</main>
</body>
</html>
"#;

pub fn index() -> Response {
    Html(PLACEHOLDER).into_response()
}
