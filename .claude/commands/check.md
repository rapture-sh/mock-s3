---
description: Run the full local check suite
---

Run these in order and report what fails, with the relevant output:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
```

If anything under `web/` changed, also run `pnpm lint` and `pnpm build` in that
directory.

Fix anything mechanical (formatting, import order, an obvious clippy suggestion)
yourself; `cargo fmt --all` handles the first category. For real failures, explain the
cause before changing behaviour, and if a conformance test fails, assume mock-s3 is
wrong about S3 before assuming the test is.
