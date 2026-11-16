mock-s3
=======

An S3-compatible object storage server for local development and testing. It speaks
enough of the S3 HTTP API that the official AWS SDKs cannot tell the difference, stores
objects in a mounted volume, and ships as a single static binary in a container that
starts in milliseconds.

Written in Rust and linked statically against musl, so the published image is the binary
and nothing else: no shell, no package manager, no distribution underneath, and 2 MB to
pull. A second port serves a web console, baked into the binary: browse buckets and
objects as folders, create and delete buckets, upload files, and watch the requests your
client is actually sending.

> **This is not a deployment target.** mock-s3 is for local development environments and
> CI. It accepts any credentials, never verifies signatures, exposes an unauthenticated
> admin API, and makes no durability guarantee. Bind it to localhost or a container
> network and keep it there. See [SECURITY.md](SECURITY.md).

We used a mocked S3 internally for a long time and kept running into its limits, in
local testing and in plenty of other places besides. mock-s3 is the replacement: a
standalone protocol engine with many buckets per instance, multipart uploads, bucket
versioning and a request log.


Running it
----------

Images are published for `linux/amd64` and `linux/arm64`, so an Apple Silicon machine
runs native code rather than emulation.

```bash
docker run --rm -p 9000:9000 -p 9001:9001 -v mock-s3-data:/data raptureoss/mock-s3:latest
```

S3 API on `:9000`, admin API and console on `:9001`. Objects live in the volume mounted
at `/data`, so they survive a container restart and you can inspect them from the host.

The second port is optional: `--no-console` drops it without touching the S3 API. It
serves the console and the JSON admin API the console reads, so anything the console can
do is scriptable with `curl` against `/_admin`.

```yaml
services:
  s3:
    image: raptureoss/mock-s3:latest
    ports:
      - "9000:9000"
      - "9001:9001"
    volumes:
      - mock-s3-data:/data

volumes:
  mock-s3-data:
```

There is a ready-made [docker-compose.yml](docker-compose.yml) in this repository.

Run without a volume and the container keeps its data in its own writable layer, which
disappears with the container. For a suite that wants nothing on disk at all, pass
`--in-memory`.


Pointing a client at it
-----------------------

mock-s3 serves **path-style** addressing (`http://host:9000/bucket/key`), because
virtual-hosted style needs wildcard DNS. Every SDK has a switch for it. Any access key
works. Signatures identify the caller but are never verified.

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
| `--base-domain <host>` | off | Also serve virtual-hosted addressing, so `bucket.<host>` addresses `bucket` |
| `--log <level>` | `info` | `error`, `warn`, `info`, `debug`, `trace`, or a full filter |
| `--log-format <fmt>` | `text` | `text` or `json`, one object per line |
| `--request-log-size <n>` | `1000` | Requests kept for `/_admin/requests`; `0` disables |

Every flag has a `MOCK_S3_`-prefixed environment variable equivalent, which is usually
easier in a compose file.


Storage layout
--------------

The volume mirrors the key space, so it can be browsed, diffed and seeded by hand:

```
/data/buckets/<bucket>/bucket.json           when it was created
/data/buckets/<bucket>/objects/<key>         the payload, at the key's own path
/data/buckets/<bucket>/meta/<key>.json       content type, user metadata, etag
```

So `photos/2024/cat.jpg` in bucket `assets` is the file
`/data/buckets/assets/objects/photos/2024/cat.jpg`. Open the volume and you are looking
at your test fixtures as files.

A payload dropped into `objects/` without a sidecar is still served (size, modification
time and content type are worked out from the file itself), so a volume can be
pre-seeded with assets before anything starts.

Versioning adds to that rather than changing it. `objects/<key>` stays where it is and
becomes the `null` version; everything else goes under a hidden prefix that no listing
shows:

```
/data/buckets/<bucket>/objects/.mock-s3/versions/<version>/<key>   a non-null version
/data/buckets/<bucket>/objects/.mock-s3/markers/<version>/<key>    a delete marker
```

So a volume written before versioning existed still works, and a bucket you enable
versioning on keeps serving every object it already had.

Two consequences of mirroring, both deliberate:

- A key cannot both hold data and act as a directory for other keys. `photos` and
  `photos/cat.jpg` cannot coexist, and the second request is refused rather than
  corrupting the first. Real S3 allows it; this is the cost of a browsable volume.
- A key with an empty path segment (`a//b`) is refused for the same reason. A key ending
  in `/`, which the AWS console creates as a folder marker, is stored as a marker file
  inside that directory.

Object keys are resolved into the data directory by one helper that rejects `../`,
absolute paths and drive prefixes, so no key can write outside the volume.


Architecture
------------

One instance, one endpoint, many buckets, which is the shape S3 itself has. Running a container
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

**Everything is scoped to a bucket**: objects, fault rules, statistics, configuration.
No global switches. That is what makes one shared instance safe for a parallel test
suite: a test making one bucket return `503` cannot affect a test using another. Give
each test its own bucket and they will not see each other.

Two things make it pleasant in a suite. `POST /_admin/reset` wipes everything, or
`/_admin/reset/{bucket}` empties one, far faster than restarting a container. And
`GET /_admin/requests` returns what clients actually sent, newest first:

```json
[{ "method": "GET", "path": "/my-bucket",
   "query": "list-type=2&prefix=tree&encoding-type=url",
   "status": 200, "duration_ms": 0, "access_key_id": "test",
   "request_id": "000000000000007B" }]
```

That answers the question a mock gets asked most. An SDK makes calls you never wrote (a
`GetBucketLocation` before a listing, a switch to multipart above a threshold), and this
is how you see them.

Rust users can skip the container and use `mock-s3-testkit` to spawn an instance
in-process on a random port.


Supported operations
--------------------

| Level | Operation | Status |
| --- | --- | --- |
| Service | `ListBuckets` | working |
| Bucket | `CreateBucket`, `DeleteBucket`, `HeadBucket` | working |
| Bucket | `ListObjectsV2`, `ListObjects` | working, with prefix, delimiter, `max-keys` and continuation tokens |
| Bucket | `GetBucketLocation` | working |
| Bucket | `PutBucketVersioning`, `GetBucketVersioning` | working, enabled and suspended |
| Bucket | `ListObjectVersions` | working, with prefix, delimiter and both page markers |
| Bucket | `GetBucketAcl`, `GetBucketRequestPayment` | working |
| Bucket | `PutBucketCors`, `GetBucketCors`, `DeleteBucketCors`, and `OPTIONS` preflight | working |
| Bucket | `DeleteObjects` | working |
| Object | `PutObject`, `GetObject`, `HeadObject`, `DeleteObject`, `CopyObject` | working, with `versionId` |
| Object | `GetObject` with a single `Range` | working |
| Object | `If-Match`, `If-None-Match`, `If-Modified-Since`, `If-Unmodified-Since` | working on `GET` and `HEAD`; not evaluated on a write |
| Object | `GetObjectAcl`, `GetObjectTagging` | working |
| Multipart | `CreateMultipartUpload`, `UploadPart`, `CompleteMultipartUpload`, `AbortMultipartUpload`, `ListParts`, `ListMultipartUploads` | working |

Multipart matters more than its place in that list suggests: SDKs switch to it on their
own above roughly 8 MB, so a caller who never mentions multipart still depends on it. A
multipart object reports the ETag S3 reports, a digest over the parts' digests with a
`-N` suffix, not the MD5 of the assembled bytes.

The bucket settings clients probe on connect (lifecycle, policy, tagging, encryption,
website, replication, object lock, notification, logging, accelerate) answer the way
real S3 does when nothing is configured, with the specific "not configured" error or an
empty document. mock-s3 stores none of those: a `PUT` of one is accepted and ignored,
because pretending to enforce a lifecycle rule would be worse than refusing.

**CORS is the exception, and is stored for real**, because a browser cannot talk to the
server at all without it. Rules are evaluated on preflight (origin, method and
requested headers, including `https://*.example.com` wildcards), and the
`Access-Control-*` headers go on ordinary responses too, which is the half that is easy
to miss. With no configuration stored, a preflight is refused, exactly as S3 refuses it;
a mock that allowed everything would let a web app pass here and fail against AWS.

**Bucket versioning works**, enabled or suspended, with version ids, delete markers and
a paginated `?versions` listing. Enabling it on a bucket that already holds objects
changes nothing on disk: those objects become the `null` version, which is what real S3
calls them too. A delete then leaves a marker rather than removing anything, so the key
reads as a `404` carrying `x-amz-delete-marker` while its earlier versions stay readable
by id, and deleting the marker puts the object back.

Two divergences worth knowing:

- A `versionId` that does not exist answers `404 NoSuchVersion`. Real S3 separates a
  malformed id (`400 InvalidArgument`) from a well-formed one that names nothing; version
  ids here are opaque, so there is no malformed to recognise.
- `DELETE /{bucket}?versioning` answers `501 NotImplemented`. S3 has no request that
  un-versions a bucket, so there is nothing to match; answering it explicitly is what
  stops it falling through to `DeleteBucket` and destroying a bucket on a request that
  asked for nothing of the kind.

**Not implemented**, and worth reading before you point a test suite at this:

- **No ACL, policy or signature enforcement.** Any credentials are accepted. A test that
  asserts a request is *refused* will not get what it expects. That is rule 7, not an
  oversight.
- **No encryption.** `SSE-C` and `SSE-KMS` are absent by design; see `SECURITY.md`.
- **Conditional *writes*.** `If-Match` and `If-None-Match` work on `GET` and `HEAD`; on
  `PUT`, copy and delete they are not evaluated, so there is no atomic create yet.
- **Object lock, retention and legal hold**, **bucket logging**, **lifecycle
  expiration**, and **object tagging stored for real**: the settings above are accepted
  and ignored, so nothing acts on them.
- **Browser form POST uploads**, `UploadPartCopy`, `GetObjectAttributes`, `S3 Select`,
  `partNumber` on `GET`, `ListBuckets` pagination, `Expect: 100-continue`, MFA delete,
  and the 1000-key limit on `DeleteObjects`.

Anything genuinely missing answers `501 NotImplemented` rather than pretending to
succeed, so a test that needs it fails loudly instead of passing against a lie. The one
exception is the list above's first two entries: a request that *should* be refused is
allowed instead, because there is nothing here to refuse it with.

[`tests/s3-tests/known-failures.txt`](tests/s3-tests/known-failures.txt) is the exact
score against [ceph/s3-tests](https://github.com/ceph/s3-tests), grouped by the reason
each test fails. It is the honest answer to "will this work for me". If the answer turns
out to be no, see [Other options](#other-options) before working around it.

Working today alongside the operations: user metadata round-tripping through
`x-amz-meta-*`, content type preservation, MD5 ETags, `encoding-type=url` listings,
`x-amz-request-id` on every response, and bucket name validation that follows the real
rules so an invalid name fails here instead of passing locally and failing against AWS.

Streaming-signed uploads (`Content-Encoding: aws-chunked`) are decoded, which is what
the AWS CLI's high-level commands use. Without it, `aws s3 cp` stores the chunk framing
alongside your bytes and the ETag silently stops matching.

Out of scope: IAM and real signature verification, replication, lifecycle transitions,
storage classes beyond `STANDARD`, and anything whose only honest implementation is
"wait several hours".


Other options
-------------

[adobe/S3Mock](https://github.com/adobe/S3Mock) is the mature one, and for a lot of
projects it is the better answer. It covers more of the API than this does, it has years
of use behind it, and on the JVM it plugs straight into a test run as a Testcontainers
component, a JUnit Jupiter extension or a TestNG listener, which is a nicer integration
than pointing a client at a container. If you need coverage or you are testing a JVM
application, start there.

It runs on the JVM, and that is the part that did not suit us. A JVM image and a Spring
Boot startup are a lot to carry when every test file wants a clean instance, and the cost
shows up on every run rather than once. mock-s3 is a static binary instead, which is
most of why it exists.

The other difference is the console. S3Mock ships no UI, so seeing what your code wrote
means listing the bucket from a client or digging through the volume. mock-s3 serves a
dashboard on its second port: buckets and their sizes, objects as folders, a click to
preview an image or a JSON file, and a live log of the requests your SDK actually sent.
When a test uploads the wrong thing to the wrong key, that is usually faster than
reaching for the CLI.

So: S3Mock if you want maturity and breadth, especially on the JVM. mock-s3 if you want
something small that starts instantly and lets you look at what happened.


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

`cargo run` is enough for server work: debug builds read console assets from `web/dist`
at runtime, so the server starts whether or not you have built the frontend. `xtask dist`
sequences the frontend build and the release build, which embeds the assets; use it
rather than `cargo build --release` on its own. For console work, `pnpm dev` in `web/`
runs Vite on `:5173` and proxies the admin API to `:9001`.

Layout, conventions, testing and style are in [AGENTS.md](AGENTS.md).


Contributions
-------------

Welcome, and you do not need permission to start. The `501` rows in the table above are
the most useful place to begin.

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md) first. The rule that
outranks the rest: match real S3, or document the divergence.

Three suites back that up: the Rust SDK against an in-process server
(`cargo test`), the JavaScript SDK against the container (`pnpm test` in `tests/`), and
the AWS CLI itself (`tests/cli/smoke.sh`). On top of those,
[`tests/s3-tests/run.sh`](tests/s3-tests/run.sh) runs the
[ceph/s3-tests](https://github.com/ceph/s3-tests) compatibility suite and compares the
result against a known-failures list that is only allowed to shrink. Commits are `feat:` or
`fix:` and at most two sentences, one logical change per PR, and anything adding protocol
surface comes with a conformance test driven through the real AWS SDK.

Conduct is covered by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md). Security issues go
through [SECURITY.md](SECURITY.md), not a public issue.


Licensing
---------

[MIT](LICENSE).

Amazon S3 is a trademark of Amazon Web Services. This project is not affiliated with or
endorsed by AWS; "S3-compatible" describes the HTTP API it implements, nothing more.
