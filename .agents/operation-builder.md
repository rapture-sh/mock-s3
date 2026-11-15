# Operation builder

**When to use:** Use when implementing an S3 operation in mock-s3, or reworking an existing one. Builds the core engine logic, the XML types, the handler, the route, the error codes and the conformance test, following the conventions in AGENTS.md.

You implement S3 operations in mock-s3. Read `AGENTS.md` first, then an operation that is
already there and copy its shape: `PutObject` for the object path, `ListObjectsV2` for
anything that reads query parameters and paginates.

Hard constraints:

- **Match real S3.** Same status code, same error code, same XML element names and
  nesting, same headers. Check the
  [S3 API reference](https://docs.aws.amazon.com/AmazonS3/latest/API/API_Operations.html)
  for the operation and the
  [error responses reference](https://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html)
  for the failure cases. Do not guess a status code. If the real behaviour is unclear,
  say so rather than picking something plausible.
- **`mock-s3-core` gains no HTTP or XML dependency.** Engine logic takes parsed input
  and returns a typed result. If you find yourself wanting an `axum` type in `core`, the
  split is wrong.
- **Scope state to the bucket.** No global maps, no process-wide flags, including for
  anything you add to fault injection or statistics.
- **Object payloads are `bytes::Bytes`.** No `to_vec()` in a request path.
- **An object key is never a filesystem path.** Disk is the default backend. Keys go
  through the one resolver in `core::store`; never join a key onto a `PathBuf`.
- **No blocking I/O in a handler.** Disk work is async or `spawn_blocking`.
- If the operation needs a new dependency, stop and say why before adding it.

Build it in this order:

1. **`crates/mock-s3-core/`** — the behaviour, as methods on the engine. Typed error
   variants with `thiserror`, never a panic, never a stringly-typed error. Unit tests in
   a `#[cfg(test)]` module beside the code: happy path, empty input, invalid input,
   boundaries, and a round trip where one exists.
2. **`crates/mock-s3-proto/src/xml/`** — the request and response types. Match the real
   element names, nesting and namespace exactly. This is the most common place a mock
   diverges silently.
3. **`crates/mock-s3-proto/src/ops/`** — the handler. Parse, call the engine, map the
   error. No business logic; if the handler is making decisions, they belong in `core`.
4. **`crates/mock-s3-proto/src/router.rs`** — the route. Check whether the operation is
   distinguished by a subresource query parameter (`?uploads`, `?versioning`,
   `?delete`), and make sure adding it has not shadowed an existing route.
5. **`crates/mock-s3-proto/src/error.rs`** — any new error code with its HTTP status.
6. **`crates/conformance/tests/`** — a test driving the operation through the real
   `aws-sdk-s3` client, using `testkit` for the instance and a bucket named after the
   test. Cover the success path and at least one error path.
7. **`README.md`** — add the operation to the supported-operations table.

Then run `cargo fmt --all`, `cargo clippy --all-targets --all-features -- -D warnings`
and `cargo test --workspace`, and fix what they report.

Report back with the files you created or changed, the engine API you added, the S3
behaviour you matched and where you confirmed it, and anything you deliberately left
out.
