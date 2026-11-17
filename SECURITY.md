# Security Policy

## Reporting a vulnerability

Please do **not** open a public issue for a security problem.

Report it privately through GitHub:
[Security → Report a vulnerability](https://github.com/rapturedotsh/mock-s3/security/advisories/new).

Include what you found, how to reproduce it, and what an attacker could do with
it. Expect an acknowledgement within a few days.

## Read this first

mock-s3 is a **testing tool with no security model**. It accepts any credentials,
does not verify signatures, and exposes an unauthenticated admin API that can
wipe all state. That is by design: it exists so test suites do not need real
credentials.

**mock-s3 is not a deployment target.** It belongs on a developer machine or in
CI. Images are published to a registry so those environments can pull them, not
so it can be hosted. Bind it to localhost or a container network and keep it
there.

Objects written to the data volume are not durable and carry no consistency
guarantee. The volume exists so state survives a container restart and can be
inspected, not so anything important can live in it.

So the following are **not** vulnerabilities, and reports about them will be
closed:

- Signatures are not verified; any access key works.
- The admin API needs no authentication.
- There is no encryption at rest, no TLS, and no audit log.
- Objects are readable by anyone who can reach the port.

## What does count

Anything that lets a caller escape the boundaries mock-s3 is supposed to keep,
or that turns a test fixture into a foothold on the developer's machine:

- **Path traversal through an object key.** An object key is an arbitrary string,
  and `../`, absolute paths, Windows drive prefixes and encoded separators are
  all legal keys. Any key that reads or writes outside the data directory is the
  most serious class of bug here, and disk is the default backend, so it is on
  the common path rather than behind a flag.
- **Cross-site scripting in the console.** Object keys, bucket names and user
  metadata are attacker-controlled strings rendered in a web page.
- **Remote code execution or memory unsafety** in request parsing, including
  anything reachable through `unsafe` or a dependency.
- **Unbounded resource consumption** from a single small request, a request that
  makes the process allocate without limit or spin forever. Uploading a large
  object and filling the volume is working as intended; a small request that does
  it is not.
- **State leaking across buckets** where the design says it is scoped: fault
  rules or objects visible from a bucket that should not see them. This breaks
  test isolation, which is a correctness promise.
- **A dependency that exfiltrates data or runs unexpected code.**

## Supported versions

The latest published image and `main` are supported. There are no backported
fixes to older tags.
