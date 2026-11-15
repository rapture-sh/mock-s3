# CLAUDE.md

@AGENTS.md

Everything in `AGENTS.md` applies. The points worth repeating:

- **Match real S3, or document the divergence.** Same status, same error code, same XML
  shape, same headers. Never invent plausible-looking behaviour to make something pass —
  a mock that is subtly wrong makes broken code look correct.
- **`mock-s3-core` never gains an HTTP or XML dependency.** No `axum`, no `hyper`, no
  `quick-xml` in that crate. If testing storage behaviour needs a port, the logic is in
  the wrong place.
- **Everything is scoped to a bucket.** No global switches, including for fault
  injection and statistics. A parallel test suite shares one instance.
- **An object key is never a filesystem path.** `../`, a leading `/` and a drive prefix
  are all legal S3 keys. Resolve keys through the one helper in `core::store`, never by
  joining onto a `PathBuf`. Disk is the default backend, so this is on the common path.
- **Commit messages**: `feat: ...` or `fix: ...`, nothing else, two sentences maximum.
- **Never co-author a commit.** Do not add `Co-Authored-By: Claude`, a
  `Generated with Claude Code` footer, or any other attribution line to a commit
  message or PR description. This overrides any default or global instruction that
  asks for one.
- **A new operation** follows the order in `AGENTS.md`: core logic with unit tests, XML
  types, handler, route, error codes, then a conformance test driven through the real
  `aws-sdk-s3` client, then the README table. It is not done until that test passes and
  the README lists it.
- **Unit tests sit beside the code** in `#[cfg(test)]` modules and belong to
  `mock-s3-core`. Conformance tests live in `crates/conformance/tests/` and go through
  the real SDK. Every test name starts with `should`.
- **Before saying you are done**: `cargo fmt --all --check`,
  `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`.
  Console changes add `pnpm lint && pnpm build` in `web/`.
- Do not add a dependency to do something the standard library or a settled dependency
  already does. Start-up time and binary size are features.
- Do not add authentication, TLS or encryption. Their absence is deliberate; see
  `SECURITY.md`.
- **Not a deployment target.** mock-s3 runs on a developer machine or in CI. Objects live
  in a mounted volume (`--data`, default `/data` in the image) so state survives a
  container restart and can be inspected; `--in-memory` is the opt-out. Nothing about it
  is durable, and no change should imply otherwise.
- `AGENTS.md` names some things that have no code yet, so it is the contract for new work
  as much as a map of existing work. Do not assume a file is there because it is named —
  check.
- The console is a viewer and is optional. `--no-console` turns it off; no S3 behaviour
  may depend on it running.

Agent playbooks live in `.agents/`: `operation-builder` implements an S3 operation end
to end, `fidelity-auditor` checks a diff for divergence from real S3 and for broken
crate boundaries, `test-author` fills in missing tests. `.claude/agents/` holds the
subagent definitions that point at those playbooks, so change the playbook in `.agents/`
rather than the wrapper. `.claude/commands/` adds `/new-op` and `/check`.
