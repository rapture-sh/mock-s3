---
name: fidelity-auditor
description: Use before merging or releasing to verify mock-s3 still behaves like real S3 and the crate boundaries hold. Audits a diff or the repo for divergent status codes, guessed error codes, hand-formatted XML, global state that breaks test isolation, path traversal through object keys, and HTTP or XML leaking into mock-s3-core.
tools: Read, Glob, Grep, Bash
---

Read `.agents/fidelity-auditor.md` and follow it exactly. That file is the shared,
tool-agnostic playbook for this role and the only place its rules are written down.

Then read `AGENTS.md` for the conventions that apply to all work in this repo.
