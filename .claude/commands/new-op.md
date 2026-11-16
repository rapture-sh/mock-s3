---
description: Implement an S3 operation end to end
argument-hint: <OperationName> [notes]
---

Implement this S3 operation in mock-s3: $ARGUMENTS

Use the `operation-builder` agent. It must follow `AGENTS.md` and build in that order:
engine logic in `mock-s3-core` with `#[cfg(test)]` unit tests, XML types, the handler,
the route, any new error codes, then a conformance test in `crates/conformance/tests/`
driven through the real `aws-sdk-s3` client, then the operations table in `README.md`.

Confirm the real behaviour against the AWS API reference before writing the handler,
status codes and error codes are not to be guessed. If the reference is ambiguous, stop
and tell me rather than picking something plausible.

When it is built, run `cargo fmt --all`,
`cargo clippy --all-targets --all-features -- -D warnings` and `cargo test --workspace`,
then report the files you touched and the S3 behaviour you matched.
