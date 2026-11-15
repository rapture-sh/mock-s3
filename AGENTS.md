# AGENTS.md

Guide for humans and AI agents working in this repo. Read this before writing code.

## What mock-s3 is

mock-s3 is an S3-compatible object storage server for local development and testing. It
speaks enough of the S3 HTTP API that the official AWS SDKs cannot tell the difference,
stores objects in a mounted volume, and ships as a single static binary in a container
that starts in milliseconds. Test suites point at it instead of a real bucket.

It is **not a deployment target.** It runs on a developer machine or in CI, and nowhere
else. Published images go to a registry so those environments can pull them, not so it
can be hosted.

It is part of [Rapture.sh](https://rapture.sh) and is MIT licensed.

**What is in the tree right now.** Buckets, objects, listings, ranges, conditional
requests, multipart uploads, `CopyObject`, `DeleteObjects`, the bucket settings clients
probe, `aws-chunked` uploads, request ids and a request log all work, verified by the
Rust SDK suite in `crates/conformance`, the JavaScript SDK suite in `tests/` and the AWS
CLI script in `tests/cli/`. Bucket versioning and fault injection have no code, and
neither does the console frontend — the console serves a placeholder page over a working
admin API. Where this file names something that has not landed, it is the contract for
building it rather than code you can read, so check that a file exists before assuming
it.

**The console is optional.** It is a viewer and nothing more: `--no-console` turns it off
and the S3 API is unaffected. No S3 behaviour may ever depend on it being there.

Published images go to the GitHub Container Registry as `ghcr.io/rapture-sh/mock-s3`.
Public images there have no anonymous pull limit, which matters for a tool that CI pulls
on every run.

## The rules that matter

1. **Match real S3, or document the divergence.** Same status code, same error code,
   same XML shape, same headers. A mock that is subtly wrong is worse than no mock,
   because it makes broken code pass. When matching S3 is genuinely impossible — real
   IAM, replication, multi-hour lifecycle transitions — list it as out of scope in the
   README rather than inventing plausible-looking behaviour.
2. **`mock-s3-core` knows nothing about HTTP or XML.** No `axum`, no `hyper`, no
   `quick-xml` in its dependencies. It owns storage, listings, multipart state and fault
   rules, and reaches the filesystem through the `Store` trait. `tokio` for async I/O is
   fine; a web framework or an XML parser is not. If testing a storage change requires
   binding a port, the logic is in the wrong crate.
3. **Everything is scoped to a bucket.** Objects, fault rules, statistics,
   configuration. No global switches, ever. A single instance has to be safe for a
   parallel test suite: one test making a bucket return `503` must not affect a test
   working in another bucket.
4. **Start-up time and binary size are features.** They are most of why this exists.
   Prefer the standard library and the settled dependencies over adding another one.
5. **Never block the async runtime.** No synchronous file I/O and no long CPU work in a
   request handler. Disk is the default backend, so this is on the common path, not an
   edge case: use async I/O or `spawn_blocking`.
6. **An object key is never a path.** Keys are arbitrary strings up to 1024 bytes, and
   `../`, a leading `/` and a Windows drive prefix are all legal S3 keys. They are
   resolved into the data directory by the one helper in `core::store` that does it
   safely, never by joining onto a `PathBuf`. Now that disk is the default, this is the
   sharpest bug class in the repo.
7. **This is a test tool, not production storage.** Any credentials are accepted and
   signatures are never verified. Do not add authentication, TLS or encryption, and do
   not treat their absence as a bug. Objects in the volume are not durable and carry no
   consistency guarantee. See `SECURITY.md`.

## Stack

- Rust, edition 2024, version pinned in `rust-toolchain.toml`
- `tokio` + `axum` + `hyper` for the HTTP surface
- `bytes` for object payloads; the S3 XML is written by hand in `mock-s3-proto::xml`,
  which is the only module allowed to produce it, so the exact shapes stay in one place
- `clap` for the CLI, `tracing` for logs
- `thiserror` for typed errors
- `rust-embed` for the console assets
- Vite + React + TypeScript for the console, Biome for its lint and format
- `aws-sdk-s3` as a dev-dependency, to drive the conformance suite

## Commands

```bash
cargo run                    # server on :9000, console on :9001
cargo test --workspace       # all tests
cargo fmt --all              # format
cargo fmt --all --check      # format check
cargo clippy --all-targets --all-features -- -D warnings
cargo xtask dist             # console build, then release binary
cargo xtask docker           # container image
```

In `web/`:

```bash
pnpm install
pnpm dev                     # Vite on :5173, proxies the admin API to :9001
pnpm build                   # writes web/dist
pnpm lint                    # biome check
pnpm lint:fix                # biome check --write
```

Before you call work done:
`cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --workspace`.
Console changes add `pnpm lint && pnpm build` in `web/`.

## Layout

```
crates/
  mock-s3-core/              the engine. no HTTP, no XML.
    src/model.rs             Bucket, ObjectKey, ETag, Metadata
    src/listing.rs           prefix and delimiter to common prefixes
    src/multipart.rs         upload id state machine
    src/fault.rs             per-bucket fault rules
    src/store/mod.rs         the Store trait, and safe key to path resolution
    src/store/disk.rs        default backend, writes under --data
    src/store/memory.rs      --in-memory backend

  mock-s3-proto/             the S3 wire protocol
    src/router.rs            path-style and virtual-hosted dispatch
    src/sigv4.rs             parse Authorization, identify the caller
    src/error.rs             engine errors to S3 error codes and XML
    src/xml/                 request and response types
    src/ops/service.rs       ListBuckets
    src/ops/bucket.rs        bucket operations
    src/ops/object.rs        object and multipart operations

  mock-s3-console/           the :9001 listener
    src/admin.rs             admin JSON API: buckets, reset, faults, stats
    src/assets.rs            rust-embed of web/dist

  mock-s3/                   the binary
    src/cli.rs               clap definitions
    src/main.rs              listener wiring

  mock-s3-testkit/           not created yet: in-process instance for a Rust test
  conformance/               not created yet: tests driven by the real aws-sdk-s3

web/                         not created yet: the console. Vite, React, TypeScript.
xtask/                       build orchestration
```

The data volume mirrors the key space, so it can be browsed and seeded by hand:

```
<data>/buckets/<bucket>/bucket.json        when it was created
<data>/buckets/<bucket>/objects/<key>      the payload, at the key's own path
<data>/buckets/<bucket>/meta/<key>.json    content type, user metadata, etag
```

Two consequences, both deliberate and both documented in the README: a key cannot both
hold data and act as a directory for other keys, and a key with an empty path segment
cannot be stored. Both are refused rather than silently mangled. A payload with no
sidecar is still served, with its metadata synthesised from the file, which is what makes
seeding a volume work — do not break that.

Where things go:

- `mock-s3-core` is the engine: input in, result out, with storage behind the `Store`
  trait. No sockets, no XML, no `axum` types. Most unit test coverage lives here, next to the
  code, because this is where the genuinely tricky parts are — `ListObjectsV2`
  pagination with prefix, delimiter and continuation tokens, and the multipart state
  machine.
- `mock-s3-proto` is the only crate that knows S3 is XML over HTTP. Handlers parse,
  call the engine, and map errors. No business logic in a handler.
- `mock-s3-console` is the admin API and the embedded assets, separate from `proto` so
  the `ui` feature gates at a crate boundary rather than with `#[cfg]` scattered
  through handlers.
- `mock-s3` is wiring only: parse arguments, build the engine, start two listeners.
  Keep `main.rs` thin.
- `conformance` holds tests only and is never published. It depends on the real AWS
  SDK, which is the point.
- `xtask` is a plain Rust binary standing in for a Makefile, so no extra tool is needed
  on any platform.

Two listeners, one engine. They are separate **ports** rather than separate paths
because in path-style addressing the URL root *is* the bucket namespace: a console
mounted at `/console` would make a bucket named `console` unreachable.

## Adding an operation

1. **`mock-s3-core`** — the behaviour, as methods on the engine, with unit tests beside
   them in a `#[cfg(test)]` module. Invalid input returns a typed error variant, never a
   panic and never a string.
2. **`mock-s3-proto/src/xml/`** — request and response types. Match the real element
   names, nesting and namespace exactly. Getting this wrong is the single most common
   way a mock diverges without anyone noticing.
3. **`mock-s3-proto/src/ops/`** — the handler: parse the request, call the engine, map
   the error. Thin.
4. **`mock-s3-proto/src/router.rs`** — wire up the route. Mind the subresource query
   parameters (`?uploads`, `?versioning`, `?delete`); they are how S3 distinguishes
   different operations on the same path and method.
5. **`mock-s3-proto/src/error.rs`** — add any new error code with its status. The
   [S3 error responses reference](https://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html)
   is the source of truth. Guessing produces a mock that retry logic cannot be tested
   against.
6. **`crates/conformance/tests/`** — a test driving the operation through the real
   `aws-sdk-s3` client. **Required for anything that adds protocol surface.**
   Hand-written HTTP proves the handler matches your understanding; the real SDK proves
   it matches what users actually send.
7. Update the operations table in `README.md`.

An operation is not done until the conformance test passes and the README lists it.

## Shared parts

Reuse these rather than building another one. They are what keeps every operation
behaving like the same server. They land with the first operations; if one is missing
when you need it, build it there rather than inline in a handler.

- `core::listing` — prefix, delimiter, `max-keys` and continuation tokens. Every listing
  operation goes through it. Do not reimplement pagination in a handler.
- `core::store::Store` — the storage trait. Disk is the default and `--in-memory` is the
  opt-out, so every operation has to work on both. New storage behaviour is a method
  here, not a branch on the backend type. Key-to-path resolution lives here too, and is
  the only place allowed to turn an object key into a filesystem path.
- `core::fault` — not built yet. Fault rules will be consulted once per request, early,
  before any work. An operation that does not check them cannot be tested for retry
  behaviour.
- `proto::error` — the one place engine errors become S3 error codes. A handler never
  writes an error body itself.
- `proto::xml` — the serialisation types. S3 responses are not ad-hoc strings; never
  format XML by hand in a handler.
- `testkit` — not built yet. It will spawn an in-process instance on a random port, so
  Rust tests do not shell out to a container.

One rule comes with them: **object payloads are `bytes::Bytes`, never `Vec<u8>`.**
Cloning `Bytes` bumps a refcount, so serving one object to many readers copies nothing.
A `to_vec()` in a hot path is a bug.

## Code style

- `rustfmt` decides formatting; run `cargo fmt --all` rather than arguing with it.
- **Clippy warnings are errors.** If a lint is genuinely wrong, `#[allow]` it with a
  comment explaining why, in the narrowest scope that works.
- **No `unsafe`.** If you think you need it, open an issue first.
- No `unwrap()` or `expect()` outside tests, build scripts and `main`. Model failure as
  a typed error with `thiserror` and handle it at the boundary.
- Public items that cross a crate boundary are documented; everything else is private by
  default. Do not make something `pub` to test it — test it from inside the module.
- Name operations exactly after the S3 operation they implement: `PutObject` becomes
  `put_object`. Someone reading the AWS docs should be able to find the code by name.
- Import groups in order: `std`, external crates, then `crate`. `rustfmt` keeps them
  tidy; do not hand-roll a different arrangement.
- Error messages are for humans: say what is wrong with the request, in plain words.
- Log with `tracing`, never `println!`. One span per request, and never log a request
  body or object payload.

For the console, `web/` follows the same spirit as the rest of Rapture.sh: Biome decides
formatting, named exports, no blank lines inside JSX, no `any`, label every input.
The console is a viewer — it holds no logic the server could hold instead.

## Comments

- Code should read on its own. Do not write a comment that repeats what the next line
  already says, and do not use banner comments to label sections.
- Use rustdoc (`///`) only where the code cannot say it itself: a contract the types do
  not capture, an invariant the caller has to keep, a unit, the error a function
  returns, or the reason something is done an unusual way. Explain why, not what.
- **Cite S3 behaviour where it is surprising.** A one-line comment saying that S3
  returns `204` here, or that this header is case-insensitive, with a doc link, is worth
  more than any other comment in this repo. The next person will otherwise assume it is
  a bug and "fix" it.
- Nothing commented out, no `TODO` without a linked issue, no changelog inside a file.
- A stale comment is worse than no comment. If the code moves on, delete the comment.

## Testing

Two tiers, and they do different jobs.

**Unit tests** live beside the code in a `#[cfg(test)]` module, and belong to
`mock-s3-core`. They cover the logic: listing pagination, multipart transitions, key
validation, fault matching. They bind no ports and run in milliseconds. This is where
coverage counts.

**Conformance tests** live in `crates/conformance/tests/` and drive a real instance
through the real `aws-sdk-s3` client. They cover the wire: signatures parse, XML shapes
match, status codes are right, the automatic multipart switchover works. Anything that
adds protocol surface needs one.

Conventions:

- **Every test name starts with `should`**, reads as a sentence, and says the case it
  covers. Add `when` where the case needs saying.

  ```rust
  mod list_objects_v2 {
      #[test]
      fn should_return_common_prefixes_when_a_delimiter_is_given() {}

      #[test]
      fn should_reject_a_max_keys_above_the_s3_limit() {}
  }
  ```

  A `mod` names the thing under test, so an operation or a type, and carries no
  `should`.

- Use `testkit` to get an instance, never a hardcoded port. Tests run in parallel and a
  fixed port makes them flaky.
- Tests use the in-memory backend unless the test is about disk behaviour. Storage tests
  that need disk get their own temporary directory and clean it up.
- Give each test its own bucket, named after the test. That is the isolation mechanism;
  rule 3 exists to make it work.
- Assert the error **code**, not the message text. `NoSuchKey` is the contract; the
  wording is not.

Tests have to earn their place. A test is worth keeping only if it can fail for a real
reason:

- Test behaviour, not structure: what a caller actually gets back. Cover the happy path,
  empty input, invalid input, boundaries, round trips, and any bug that was just fixed.
- No test that restates the implementation or asserts a mock was called with the
  argument you just handed it.
- Do not test `tokio`, `axum` or the AWS SDK. Assume they work.
- Avoid arbitrary thresholds. Assert the invariant you actually care about.
- A few tests that would catch a real break beat many that pad the count.
- Test files follow everything else in this file, the comment rules included.

## Benchmarks

Performance claims need numbers. If a change is meant to be faster, say what you
measured, how, and the before and after. "Should be faster" is not a benchmark, and a
microbenchmark of something that happens once per process start is not a result.

## Agent playbooks

`.agents/` holds one playbook per role in plain Markdown, with no vendor frontmatter, so
any coding agent can be pointed at them:

- `.agents/operation-builder.md` implements an S3 operation end to end.
- `.agents/fidelity-auditor.md` audits a diff for divergence from real S3 and for
  anything that breaks the crate boundaries.
- `.agents/test-author.md` writes the tests that are missing.

For Claude Code, `.claude/agents/` wraps each playbook as a subagent and
`.claude/commands/` adds `/new-op` and `/check`. The instructions themselves stay in
`.agents/` only, so nothing has to be kept in sync.

Rules that apply to everyone belong in this file, not in a playbook.

## Git

- Branch from `main`: `feat/<short-name>` or `fix/<short-name>`.
- **Commit messages are `feat: ...` or `fix: ...` only, and at most two sentences.**
  No other prefixes, no bodies that ramble, no emoji.

  ```
  feat: add UploadPartCopy with range support
  fix: return NoSuchUpload instead of 500 for an unknown upload id
  ```

- **No co-author trailers, no tool attribution.** A commit message is the one
  `feat:`/`fix:` line and nothing else. Never append `Co-Authored-By:` trailers,
  `Generated with ...` footers, or any other credit for an AI agent, editor or tool.
  Not in commit messages, not in PR descriptions. The human who opened the PR is the
  author.
- One logical change per PR. Keep the diff readable.
- `Cargo.lock` is committed. This is a binary, not a library.
