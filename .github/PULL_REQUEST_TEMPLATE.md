## What this changes

<!-- One or two sentences. Link the issue if there is one. -->

## Why

<!-- What problem does it solve? If this changes how mock-s3 answers a request, say what
real S3 does and link the AWS documentation that says so. -->

## Checklist

- [ ] `cargo fmt --all --check`
- [ ] `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Console changes also pass `pnpm lint` and `pnpm build` in `web/`
- [ ] Commit messages are `feat: ...` or `fix: ...`, two sentences at most
- [ ] Any new dependency is named below with a reason the standard library or an existing
      one cannot cover it

## If this adds or changes protocol surface

<!-- Delete this section if it does not. -->

- [ ] Matches real S3 — same status, same error code, same XML shape, same headers — or
      the divergence is documented in `README.md` and says why
- [ ] A conformance test in `crates/conformance/tests/` drives it through the real
      `aws-sdk-s3` client
- [ ] A bug fix has a regression test that fails without the fix
- [ ] The operations table in `README.md` is up to date
- [ ] `mock-s3-core` gained no HTTP or XML dependency

## New dependencies

<!-- None, or: the crate, what it does, why the standard library or a settled dependency
cannot, and what it costs in compile time. -->
