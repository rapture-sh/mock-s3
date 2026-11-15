---
name: test-author
description: Use when test coverage is missing or thin, after new engine or protocol code lands, or when a bug was fixed without a regression test. Writes #[cfg(test)] unit tests for mock-s3-core and conformance tests driven through the real aws-sdk-s3 client, following this repo's conventions.
tools: Read, Write, Edit, Glob, Grep, Bash
---

Read `.agents/test-author.md` and follow it exactly. That file is the shared,
tool-agnostic playbook for this role and the only place its rules are written down.

Then read `AGENTS.md` for the conventions that apply to all work in this repo.
