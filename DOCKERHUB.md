# mock-s3

An S3-compatible object storage server for local development and testing. It speaks
enough of the S3 HTTP API that the official AWS SDKs cannot tell the difference, stores
objects in a mounted volume you can browse, and ships as a single static binary that
starts in milliseconds.

Written in Rust. No JVM, no runtime dependencies, around 30 MB. A second port serves an
optional web console, baked into the same binary, so you can see what your code actually
uploaded without reaching for the CLI. Turn it off with `--no-console` and the S3 API is
unchanged.

> **Not a deployment target.** mock-s3 is for development machines and CI. It accepts any
> credentials, never verifies signatures, exposes an unauthenticated admin API, and makes
> no durability guarantee. Bind it to localhost or a container network and keep it there.

## Quick start

```bash
docker run --rm -p 9000:9000 -p 9001:9001 -v mock-s3-data:/data raptureoss/mock-s3
```

S3 API on `:9000`, web console and admin API on `:9001`. Objects live in the volume at
`/data`, so they survive a restart and you can open them from the host.

For a suite that wants nothing on disk, drop the volume and add `--in-memory`.

## Tags and architectures

`latest` tracks releases, never the main branch. In CI, pin a major such as `:1` so
patches reach you but a new major cannot land on you by surprise.

Built for `linux/amd64` and `linux/arm64`, so an Apple Silicon machine runs native code
rather than emulation.

## Pointing a client at it

Two things matter and nothing else does. Use **path-style** addressing, because
virtual-hosted style needs wildcard DNS. And use **any credentials at all**, because
signatures identify the caller but are never verified, so there is nothing to get wrong.

```ts
const s3 = new S3Client({
  endpoint: "http://localhost:9000",
  region: "us-east-1",
  forcePathStyle: true,
  credentials: { accessKeyId: "test", secretAccessKey: "test" },
})
```

```python
boto3.client("s3", endpoint_url="http://localhost:9000")
```

```bash
aws --endpoint-url http://localhost:9000 s3 ls
```

The Rust, Go and Java SDKs have the same two knobs.

## Compose

```yaml
services:
  s3:
    image: raptureoss/mock-s3:1
    ports:
      - "9000:9000"
      - "9001:9001"
    volumes:
      - mock-s3-data:/data

volumes:
  mock-s3-data:
```

Add a health check if other services wait on it:

```yaml
    healthcheck:
      test: ["CMD", "/usr/local/bin/mock-s3", "--version"]
      interval: 5s
      retries: 5
```

## Configuration

Every setting is a flag and an environment variable. In a compose file the variable is
usually easier.

| Variable | Flag | Default | What it does |
| --- | --- | --- | --- |
| `MOCK_S3_DATA` | `--data` | `/data` | Where objects are written |
| `MOCK_S3_IN_MEMORY` | `--in-memory` | `false` | Keep everything in RAM and discard it on exit |
| `MOCK_S3_PORT` | `--port` | `9000` | S3 API port |
| `MOCK_S3_CONSOLE_PORT` | `--console-port` | `9001` | Console and admin API port |
| `MOCK_S3_NO_CONSOLE` | `--no-console` | `false` | Do not start the console listener |
| `MOCK_S3_BIND` | `--bind` | `0.0.0.0` | Address both listeners bind to |
| `MOCK_S3_REGION` | `--region` | `us-east-1` | Region reported in responses |
| `MOCK_S3_BASE_DOMAIN` | `--base-domain` | unset | Also serve virtual-hosted addressing, so `bucket.<host>` addresses `bucket` |
| `MOCK_S3_LOG` | `--log` | `info` | `error`, `warn`, `info`, `debug`, `trace`, or a full filter |
| `MOCK_S3_LOG_FORMAT` | `--log-format` | `text` | `text`, or `json` for one object per line |
| `MOCK_S3_REQUEST_LOG_SIZE` | `--request-log-size` | `1000` | Requests kept for `/_admin/requests`. `0` disables |

The two boolean variables take `true` or `false` and nothing else. `MOCK_S3_IN_MEMORY=1`
is refused at startup rather than quietly ignored.

## The volume

The data directory mirrors the key space, so you can browse, diff and seed it by hand:

```
/data/buckets/<bucket>/bucket.json       when it was created
/data/buckets/<bucket>/objects/<key>     the payload, at the key's own path
/data/buckets/<bucket>/meta/<key>.json   content type, user metadata, etag
```

So `photos/2024/cat.jpg` in bucket `assets` is the file
`/data/buckets/assets/objects/photos/2024/cat.jpg`. Open the volume and you are looking
at your test fixtures as files.

Drop a file into `objects/` with no sidecar and it is still served, with its size,
modification time and content type worked out from the file, which is what makes seeding
a volume work.

Two consequences, both deliberate. A key cannot both hold data and act as a directory for
other keys, so `photos` and `photos/cat.jpg` cannot coexist. And a key with an empty path
segment (`a//b`) is refused. Both are refused outright rather than silently mangled.

## The console

Port `:9001` serves a dashboard: buckets and their sizes, objects as folders, a click to
preview an image or a JSON file, and a live log of the requests your SDK actually sent.
That last one answers the question a mock gets asked most, because an SDK makes calls you
never wrote: a `GetBucketLocation` before a listing, a switch to multipart above a size
threshold.

It reads a JSON admin API on the same port, so anything it does is scriptable:

```bash
curl -X POST http://localhost:9001/_admin/reset          # wipe everything
curl -X POST http://localhost:9001/_admin/reset/{bucket} # empty one bucket
curl http://localhost:9001/_admin/requests               # what clients actually sent
```

Resetting is much faster than restarting the container between tests. The console is
optional and no S3 behaviour depends on it: `--no-console` drops the listener entirely.

## What works

Buckets, objects, listings with prefix and delimiter and continuation tokens, ranged
reads, conditional reads, `CopyObject`, `DeleteObjects`, bucket versioning with delete
markers, CORS with preflight, and the full multipart API.

Multipart matters more than it sounds, because SDKs switch to it on their own above
roughly 8 MB, so a caller who never mentions multipart still depends on it. A multipart
object reports the ETag S3 reports, a digest over the parts' digests with a `-N` suffix,
not the MD5 of the assembled bytes.

Streaming-signed uploads (`Content-Encoding: aws-chunked`) are decoded, which is what the
AWS CLI's high-level commands send. Without that, `aws s3 cp` stores the chunk framing
alongside your bytes and the ETag quietly stops matching.

The bucket settings clients probe on connect (lifecycle, policy, tagging, encryption,
website, replication, object lock, notification, logging) answer the way real S3 does when
nothing is configured.

## What does not

Worth reading before you point a suite at this.

- **No ACL, policy or signature enforcement.** Any credentials are accepted, so a test
  asserting that a request is *refused* will not get what it expects. This is deliberate,
  not an oversight.
- **No encryption.** `SSE-C` and `SSE-KMS` are absent by design.
- **Conditional writes.** `If-Match` and `If-None-Match` work on `GET` and `HEAD`. On
  `PUT`, copy and delete they are not evaluated, so there is no atomic create yet.
- **Object lock, retention, legal hold, bucket logging, lifecycle expiration** and
  **object tagging stored for real**. Those settings are accepted and ignored, because
  pretending to enforce a lifecycle rule would be worse than refusing.
- **Browser form POST uploads**, `UploadPartCopy`, `GetObjectAttributes`, `S3 Select`,
  `partNumber` on `GET`, MFA delete.

Anything genuinely missing answers `501 NotImplemented` rather than pretending to
succeed, so a test that needs it fails loudly instead of passing against a lie.

Out of scope entirely: IAM, replication, lifecycle transitions, storage classes beyond
`STANDARD`, and anything whose only honest implementation is to wait several hours.

## Is it compatible enough for you

The honest answer is a file, not a claim.
[`tests/s3-tests/known-failures.txt`](https://github.com/rapture-sh/mock-s3/blob/main/tests/s3-tests/known-failures.txt)
is the exact score against [ceph/s3-tests](https://github.com/ceph/s3-tests), grouped by
the reason each test fails.

If the answer turns out to be no, [adobe/S3Mock](https://github.com/adobe/S3Mock) covers
more of the API and has years of use behind it. On the JVM it also plugs straight into a
test run as a Testcontainers component or a JUnit extension, which is a nicer integration
than pointing a client at a container. It runs on the JVM and ships no UI, which is the
part that did not suit us, but if you need coverage or you are testing a JVM application,
start there.

## Links

- Source, full README and issues: [github.com/rapture-sh/mock-s3](https://github.com/rapture-sh/mock-s3)
- Security policy: [SECURITY.md](https://github.com/rapture-sh/mock-s3/blob/main/SECURITY.md)
- Licence: [MIT](https://github.com/rapture-sh/mock-s3/blob/main/LICENSE)

Part of [Rapture.sh](https://rapture.sh).

Amazon S3 is a trademark of Amazon Web Services. This project is not affiliated with or
endorsed by AWS. "S3-compatible" describes the HTTP API it implements, nothing more.
