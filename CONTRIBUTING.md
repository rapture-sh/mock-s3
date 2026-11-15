# Contributing to mock-s3

Thanks for being here. mock-s3 is pre-release, which means there is a lot of
obvious work available and you do not need permission to start. Open an issue if
you want to talk something through first.

## The contract

Read [AGENTS.md](AGENTS.md). It is the actual contract for this repo — layout, code
style, testing conventions, and the rules below in full. Everything here assumes it.

Using an AI agent to help? Point it at `AGENTS.md` and the matching playbook in
`.agents/`. The same rules apply to code it writes, including the one about attribution
in commit messages.

One rule outranks everything else in this repo:

> **Match real S3, or document the divergence.** If the AWS SDK sends a request
> that real S3 answers one way, mock-s3 answers the same way: same status, same
> error code, same XML shape, same headers. A mock that is subtly wrong is worse
> than no mock, because it makes broken code pass.

When you genuinely cannot match S3 — because the honest implementation takes
hours, or needs real IAM — say so in the README under "out of scope" rather than
inventing plausible-looking behaviour.

The second rule is structural:

> **`mock-s3-core` knows nothing about HTTP or XML.** No `axum`, no `hyper`, no
> `quick-xml` in its dependency list. Storage, listings, multipart state and
> fault rules reach the filesystem through the `Store` trait. If testing a storage change
> requires binding a port, the logic is in the wrong crate.

## Good contributions

- **A missing operation.** Check the supported-operations table in the README.
  Anything absent and in scope is fair game.
- **A fidelity bug.** Somewhere mock-s3 returns the wrong status, error code or
  header compared to real S3. These are the most valuable reports, and even a
  report without a fix is useful.
- **Console work.** Object browsing, prefix navigation, request inspection.
- **Performance.** With a benchmark showing the before and after. Not a guess.
- **Documentation.** If something took you an hour to figure out, that is a bug
  in the README.

Not sure if an idea fits? Open an issue describing it. That is cheaper than
building it and finding out.

## Setup

Rust is pinned in `rust-toolchain.toml`; rustup picks it up automatically.

```bash
cargo run
```

Working on the console needs Node 20+ and pnpm 10+:

```bash
cd web
pnpm install
pnpm dev
```

Vite serves on `:5173` and proxies the admin API to `:9001`, so frontend changes
hot reload without touching Rust.

## Adding an operation

Follow the shape of an existing one — `PutObject` is the reference.

1. **`crates/mock-s3-core/`** — the behaviour, as functions on the engine. Unit
   tests next to the logic for the interesting state transitions and edge cases.
2. **`crates/mock-s3-proto/src/xml/`** — request and response types. Match the
   real element names and nesting exactly, including the namespace.
3. **`crates/mock-s3-proto/src/ops/`** — the handler. Parse, call the engine, map
   errors through `error.rs`. No business logic here.
4. **`crates/mock-s3-proto/src/router.rs`** — wire up the route. Mind the
   subresource query parameters (`?uploads`, `?versioning`); they are how S3
   distinguishes operations on the same path and method.
5. **`crates/conformance/tests/`** — a test driving it through the real
   `aws-sdk-s3` client. **This is required for anything that adds protocol
   surface.** Hand-written HTTP requests prove your handler matches your
   understanding; the real SDK proves it matches what users will send.
6. Update the operations table in the README.

For error cases, the AWS
[S3 error responses reference](https://docs.aws.amazon.com/AmazonS3/latest/API/ErrorResponses.html)
is the source of truth for codes and status pairings. Guessing produces mocks
that retry logic cannot be tested against.

## Dependencies

Default answer is no, and it matters more here than in most projects: start-up
time and binary size are features. A dependency needs to earn its place in the
PR description — what it does, why the standard library or an existing dependency
cannot, and what it costs in compile time.

`tokio`, `axum`, `bytes`, `clap`, `chrono` and `tracing` are settled. Adding another
async runtime or HTTP stack is not a PR, it is a discussion. The S3 XML is written by
hand in `mock-s3-proto::xml` rather than through a serialisation crate, so the output
matches S3 exactly; adding an XML serialiser needs a reason beyond tidiness.

## Checks

Run these before opening a PR, and make sure they pass:

```bash
cargo fmt --all --check
```

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

```bash
cargo test --workspace
```

Console changes also need `pnpm lint` and `pnpm build` in `web/`.

Clippy warnings are errors here. If a lint is genuinely wrong, `#[allow]` it with
a comment explaining why, in the narrowest scope that works.

## Commits

Commit messages are `feat: ...` or `fix: ...` and **at most two sentences**. No
other prefixes, no essays, no emoji.

```
feat: add UploadPartCopy with range support
fix: return NoSuchUpload instead of 500 for unknown upload id
```

No `Co-Authored-By:` trailers and no tool or AI attribution footers, in commit
messages or PR descriptions. The person opening the PR is the author.

Branches: `feat/<short-name>` or `fix/<short-name>`, cut from `main`.

## Pull requests

- One logical change per PR.
- Say what it does and why. For a fidelity fix, say what real S3 does and how you
  know — a doc link or an observed response.
- Conformance test for new protocol surface; regression test for any bug fixed.
- Note any new dependency and why it earned its place.
- Benchmark numbers for anything claiming to be faster.

Reviews are about the code, never the person. See
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## Forking

Fork it, run your own copy, build something else on top; that is what MIT is for.
Keep the copyright notice and credit where the code came from, and we are square.
