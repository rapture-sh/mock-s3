## What this changes

<!-- One or two sentences. Link the issue if there is one. -->

## Why

<!-- What problem does it solve? For a fidelity fix: what does real S3 do, and
how do you know — a doc link or an observed response. -->

## Checklist

- [ ] Behaviour matches real S3, or the divergence is documented in the README
- [ ] `mock-s3-core` still has no HTTP or XML dependencies
- [ ] `cargo fmt --all --check` passes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes
- [ ] `cargo test --workspace` passes
- [ ] New protocol surface has a conformance test driven through the real AWS SDK
- [ ] Bug fixes have a regression test
- [ ] Operations table in the README updated, if this adds or changes one
- [ ] Commit messages are `feat: ...` or `fix: ...`, two sentences at most

## Console changes

<!-- Delete if none. Screenshots required, light and dark if it differs.
`pnpm lint` and `pnpm build` pass in web/. -->

## New dependencies

<!-- None, or: the crate, what it does, why the standard library or an existing
dependency can't, and what it costs in compile time. -->

## Performance

<!-- Delete unless this claims to be faster. Numbers before and after, and how
you measured. -->
