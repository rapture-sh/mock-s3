# Contributing to mock-s3

Thanks for being here. You do not need permission to start — open an issue if you want
to talk something through first. The operations that currently answer
`501 NotImplemented`, listed in the README, are the most useful place to begin.

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

`tokio`, `axum`, `bytes`, `clap`, `chrono`, `quick-xml` and `tracing` are settled. Adding
another async runtime or HTTP stack is not a PR, it is a discussion.

`quick-xml` reads only. Response XML is written by hand in `mock-s3-proto::xml` so the
shapes match S3 exactly, and that is the one module allowed to produce it; swapping in a
serialiser needs a reason beyond tidiness.

## Measuring compatibility

`tests/s3-tests/run.sh` runs the [ceph/s3-tests](https://github.com/ceph/s3-tests) suite
against mock-s3 and compares the result with `known-failures.txt`. It is the only
outside measure of how compatible this actually is, and the list only ever shrinks — a
failure that is not in it fails the run.

Expect a long list. The suite assumes real S3 with several accounts, and mock-s3
deliberately has no account model, lifecycle or policy enforcement, so every test
covering those is expected to fail. Moving one line out of that file is a better
contribution than most features.

Every line in it sits under the reason it is expected to fail, and `--update` puts the
groups back from the patterns at the top of `run.sh` rather than discarding them. Two
things follow:

- A failure that matches no pattern lands under **not yet triaged**, which is the honest
  answer for one nobody has looked at. That section is the to-do list: shrink it by
  fixing the test or by adding a reason that is actually true. A reason invented to make
  the list look tidy is worse than no reason, because it stops anyone looking again.
- A new reason is a line in `reasons`, most specific first — the first pattern to match a
  test id claims it.

`--update` prints how many lines it removed and how many it added, and shouts about any
failure that was not already expected. A reseed that *adds* lines is raising the bar it
is meant to lower, so say why in the PR.

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

## Repository settings

The rules on this repository are kept as files rather than as clicks in a settings page,
so a change to them is reviewable like anything else.

[`.github/rulesets/main.json`](.github/rulesets/main.json) protects the default branch:
pull request required, both CI checks green, linear history, no force pushes, no
deletion, and squash as the only merge method. Zero required approvals, so a solo
maintainer is not locked out of their own repository — raise it the day there are two of
you.

Apply or update it with:

```bash
# First time
gh api -X POST repos/rapture-sh/mock-s3/rulesets --input .github/rulesets/main.json

# Afterwards, with the id from: gh api repos/rapture-sh/mock-s3/rulesets
gh api -X PUT repos/rapture-sh/mock-s3/rulesets/<id> --input .github/rulesets/main.json
```

**Apply it after the first push, not before.** A ruleset that requires a pull request on
a branch that does not exist yet turns `git push -u origin main` into a rejection with a
confusing message.

A second ruleset, applied already, protects `refs/tags/v*` from deletion and from being
force-moved. Release tags are what the published image tags are derived from, so a tag
that can be rewritten is a version number that cannot be trusted.

Squash is the only merge method on purpose: the pull request title becomes the commit
message, which is what keeps `main` to the one-line `feat:`/`fix:` convention without
anyone having to police it.

## Releasing

For maintainers. Releases are cut by pushing a tag; nothing publishes from an ordinary
push to `main`.

```bash
# 1. Bump the version in the workspace Cargo.toml, and commit it.
#    Cargo.lock changes too, so commit both. The convention is feat:/fix: and nothing
#    else, so a release commit is a feat:.
cargo build --workspace
git commit -am "feat: release 1.1.0"

# 2. Tag it. The tag is what triggers the release.
git tag v1.1.0
git push origin main v1.1.0
```

`.github/workflows/release.yml` then builds `linux/amd64` and `linux/arm64`, each on a
runner of that architecture, and publishes one multi-architecture index under four tags:

| Tag | Moves | For |
| --- | --- | --- |
| `1.1.0` | never | a pinned, reproducible pull |
| `1.1` | each patch | fixes without features |
| `1` | each minor | anything backwards-compatible |
| `latest` | each release | trying it out |

`latest` follows releases rather than `main` on purpose. Anyone who wrote
`image: raptureoss/mock-s3` in a compose file would otherwise be running whatever landed
an hour ago.

Images go to Docker Hub as `raptureoss/mock-s3`, and nowhere else. Docker Hub
rate-limits anonymous pulls, so if CI users start reporting `toomanyrequests`, mirroring
to a second registry is the fix — a dozen lines in the release workflow.

### What the workflow needs

Two repository secrets, under Settings → Secrets and variables → Actions:

| Secret | Value |
| --- | --- |
| `DOCKERHUB_USERNAME` | the Docker Hub account that owns `raptureoss` |
| `DOCKERHUB_TOKEN` | a Docker Hub personal access token with **Read & Write** |

Generate the token under Docker Hub → Account Settings → Personal access tokens. Not
your account password, and not a token with delete permission — a release never deletes
a tag. The token is shown once.

The Docker Hub repository has to exist and be public before the first release. A private
repository fails an anonymous `docker pull` with an authentication error, which reads
like a missing image rather than a permission problem.

### Building an image by hand

`cargo xtask docker` builds `raptureoss/mock-s3:dev` for the architecture you are on,
which is what the test targets use. It is deliberately single-architecture: a
multi-architecture image cannot be loaded into a local Docker daemon, only pushed, so
there is nothing useful to build locally.

## Forking

Fork it, run your own copy, build something else on top; that is what MIT is for.
Keep the copyright notice and credit where the code came from, and we are square.
