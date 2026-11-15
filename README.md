mock-s3
=======

An S3-compatible object storage server for local development and testing. It speaks
enough of the S3 HTTP API that the official AWS SDKs cannot tell the difference, stores
objects in a mounted volume, and ships as a single static binary in a container that
starts in milliseconds.

Written in Rust, with no JVM and no runtime dependencies. A static web console comes
baked into the binary, so you can see what your code just wrote.

> **This is not a deployment target.** mock-s3 is for local development environments and
> CI. It accepts any credentials, never verifies signatures, exposes an unauthenticated
> admin API, and makes no durability guarantee. Bind it to localhost or a container
> network and keep it there. See [SECURITY.md](SECURITY.md).

It started inside the [Kywi](https://github.com/rapture-sh) monorepo, standing in for S3
across that test suite. Other projects needed the same thing, so it was extracted here,
rebuilt around a standalone protocol engine and extended well past the original — many
buckets per instance, multipart uploads, per-bucket fault injection and a console. Kywi
now consumes it like everyone else.

**Status: pre-release.** This describes the target shape of the first tagged release,
not a finished product. The admin API will break without ceremony until `0.1.0`. See the
[issues](https://github.com/rapture-sh/mock-s3/issues) for what is wired up today.


Running it
----------

```bash
docker run --rm -p 9000:9000 -p 9001:9001 -v mock-s3-data:/data ghcr.io/rapture-sh/mock-s3:latest
```

S3 API on `:9000`, web console on `:9001`. Objects live in the volume mounted at
`/data`, so they survive a container restart and you can inspect them from the host.

```yaml
services:
  s3:
    image: ghcr.io/rapture-sh/mock-s3:latest
    ports:
      - "9000:9000"
      - "9001:9001"
    volumes:
      - mock-s3-data:/data

volumes:
  mock-s3-data:
```

Run without a volume and the container keeps its data in its own writable layer, which
disappears with the container. For a suite that wants nothing on disk at all, pass
`--in-memory`.


Pointing a client at it
-----------------------

mock-s3 serves **path-style** addressing (`http://host:9000/bucket/key`), because
virtual-hosted style needs wildcard DNS. Every SDK has a switch for it. Any access key
works — signatures identify the caller but are never verified.

```ts
const s3 = new S3Client({
  endpoint: "http://localhost:9000",
  region: "us-east-1",
  forcePathStyle: true,
  credentials: { accessKeyId: "test", secretAccessKey: "test" },
});
```

boto3 wants `endpoint_url` and nothing else special; the Rust, Go and Java SDKs all have
the same two knobs.


Configuration
-------------

| Flag | Default | What it does |
| --- | --- | --- |
| `--data <dir>` | `/data` in the image, `./data` otherwise | Where objects are written |
| `--in-memory` | off | Keep everything in RAM and discard it on exit |
| `--port` | `9000` | S3 API port |
| `--console-port` | `9001` | Console and admin API port |
| `--no-console` | off | Disable the console listener |
| `--region` | `us-east-1` | Region reported in responses |
| `--log <level>` | `info` | `error`, `warn`, `info`, `debug`, `trace` |

Every flag has a `MOCK_S3_`-prefixed environment variable equivalent, which is usually
easier in a compose file.


Architecture
------------

One instance, one endpoint, many buckets — the shape S3 itself has. Running a container
per bucket would be like running a database server per table.

```
mock-s3 process
├── :9000  S3 HTTP API   ──┐
│                          ├──>  engine  ──>  store  ──>  /data volume
└── :9001  console + admin ┘
```

Two listeners share one engine. They are separate **ports** rather than paths because in
path-style addressing the URL root *is* the bucket namespace: a console at `/console`
would make a bucket named `console` unreachable.

**Everything is scoped to a bucket** — objects, fault rules, statistics, configuration.
No global switches. That is what makes one shared instance safe for a parallel test
suite: a test making one bucket return `503` cannot affect a test using another. Give
each test its own bucket and they will not see each other.

Two things make it pleasant in a suite: `POST /_admin/reset` wipes everything or a
single bucket, far faster than restarting a container; and per-bucket fault injection
lets you make specific requests fail with a given status, code or latency, so testing
retry logic stops needing a broken network.

Rust users can skip the container and use `mock-s3-testkit` to spawn an instance
in-process on a random port.


Supported operations
--------------------

| Level | Operations |
| --- | --- |
| Service | `ListBuckets` |
| Bucket | `CreateBucket`, `DeleteBucket`, `HeadBucket`, `ListObjectsV2`, `ListObjects`, `GetBucketLocation`, `GetBucketVersioning` |
| Object | `PutObject`, `GetObject`, `HeadObject`, `DeleteObject`, `DeleteObjects`, `CopyObject` |
| Multipart | `CreateMultipartUpload`, `UploadPart`, `UploadPartCopy`, `CompleteMultipartUpload`, `AbortMultipartUpload`, `ListParts`, `ListMultipartUploads` |

Multipart is not optional: SDKs switch to it automatically above a size threshold with no
change in caller code, so a mock without it fails the first time somebody uploads a large
file.

Range and conditional requests, user metadata and content type round-tripping are part of
the target surface, and bucket name validation follows the real rules so invalid names
fail here instead of passing locally and failing against AWS.

Out of scope: IAM and real signature verification, replication, lifecycle transitions,
storage classes beyond `STANDARD`, and anything whose only honest implementation is
"wait several hours".


Building from source
--------------------

Needs a [Rust](https://rustup.rs) toolchain, pinned in `rust-toolchain.toml`. The console
additionally needs Node 20+ and pnpm 10+.

```bash
cargo run
```

```bash
cargo xtask dist
```

`cargo run` is enough for server work — debug builds read console assets from `web/dist`
at runtime, so the server starts whether or not you have built the frontend. `xtask dist`
sequences the frontend build and the release build, which embeds the assets; use it
rather than `cargo build --release` on its own. For console work, `pnpm dev` in `web/`
runs Vite on `:5173` and proxies the admin API to `:9001`.

Layout, conventions, testing and style are in [AGENTS.md](AGENTS.md).


Contributions
-------------

Welcome, and you do not need permission to start. The pre-release state means there is
plenty of obvious work available.

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) first. The rule that
outranks the rest: match real S3, or document the divergence. Commits are `feat:` or
`fix:` and at most two sentences, one logical change per PR, and anything adding protocol
surface comes with a conformance test driven through the real AWS SDK.

Conduct is covered by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Security issues go
through [SECURITY.md](SECURITY.md), not a public issue.


Licensing
---------

[MIT](LICENSE).

Amazon S3 is a trademark of Amazon Web Services. This project is not affiliated with or
endorsed by AWS; "S3-compatible" describes the HTTP API it implements, nothing more.
