import { PutObjectCommand } from "@aws-sdk/client-s3";
import { describe, expect, it } from "vitest";

import { consoleEndpoint, createBucket, s3, uniqueBucket } from "../support/s3";

/**
 * The admin API is what the console reads, so it is tested the way the console calls it:
 * plain fetch against the console port, parsing JSON. Errors are JSON here rather than
 * the S3 API's XML, with the same codes.
 */
async function admin(path: string, init?: RequestInit): Promise<Response> {
  return fetch(`${consoleEndpoint}${path}`, init)
}

async function adminJson<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await admin(path, init)
  if (!response.ok) {
    throw new Error(`${path} answered ${response.status}: ${await response.text()}`)
  }
  return (await response.json()) as T
}

describe("GET /_admin/health", () => {
  it("should report the region and which backend is in use", async () => {
    const health = await adminJson<{ status: string; region: string; backend: string }>(
      "/_admin/health",
    )

    expect(health.status).toBe("ok")
    expect(health.region).toBeTruthy()
    // "memory", or the data directory. Either way a developer can tell whether their
    // objects will survive a restart.
    expect(health.backend).toBeTruthy()
  })
})

describe("POST /_admin/buckets", () => {
  it("should create a bucket the S3 API can then see", async () => {
    const name = uniqueBucket("admin-create")

    const response = await admin("/_admin/buckets", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name }),
    })

    expect(response.status).toBe(201)
    const listed = await adminJson<{ name: string }[]>("/_admin/buckets")
    expect(listed.map((bucket) => bucket.name)).toContain(name)
  })

  // The engine validates against the real S3 rules, so a name the console accepts is one
  // AWS would accept. The console can rely on this rather than reimplementing it.
  it("should refuse a name real S3 would refuse", async () => {
    const response = await admin("/_admin/buckets", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: "Not-Valid" }),
    })

    expect(response.status).toBe(400)
    const body = (await response.json()) as { error: string; message: string }
    expect(body.error).toBe("InvalidBucketName")
    expect(body.message).toBeTruthy()
  })

  it("should refuse a label that starts or ends with a hyphen", async () => {
    const response = await admin("/_admin/buckets", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: "foo-.bar" }),
    })

    expect(response.status).toBe(400)
  })
})

describe("DELETE /_admin/buckets/{bucket}", () => {
  it("should delete an empty bucket", async () => {
    const bucket = await createBucket("admin-del")

    const response = await admin(`/_admin/buckets/${bucket}`, { method: "DELETE" })

    expect(response.status).toBe(204)
  })

  // Purge is a separate, deliberate action. Emptying the bucket as a side effect of
  // deleting it would lose fixtures to a misclick.
  it("should refuse a bucket that still holds objects", async () => {
    const bucket = await createBucket("admin-full")
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "a.txt", Body: "x" }))

    const response = await admin(`/_admin/buckets/${bucket}`, { method: "DELETE" })

    expect(response.status).toBe(409)
    expect(((await response.json()) as { error: string }).error).toBe("BucketNotEmpty")
  })

  it("should report a bucket that is not there", async () => {
    const response = await admin(`/_admin/buckets/${uniqueBucket("ghost")}`, {
      method: "DELETE",
    })

    expect(response.status).toBe(404)
    expect(((await response.json()) as { error: string }).error).toBe("NoSuchBucket")
  })
})

describe("GET /_admin/buckets/{bucket}", () => {
  it("should report counts, size and versioning", async () => {
    const bucket = await createBucket("admin-detail")
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "a.txt", Body: "12345" }))

    const detail = await adminJson<{
      name: string
      objects: number
      bytes: number
      versioning: string
    }>(`/_admin/buckets/${bucket}`)

    expect(detail.name).toBe(bucket)
    expect(detail.objects).toBe(1)
    expect(detail.bytes).toBe(5)
    expect(detail.versioning).toBe("Unversioned")
  })
})

describe("GET /_admin/buckets/{bucket}/objects", () => {
  it("should browse a key space as folders", async () => {
    const bucket = await createBucket("admin-browse")
    for (const key of ["photos/a.jpg", "photos/b.jpg", "notes.txt"]) {
      await s3.send(new PutObjectCommand({ Bucket: bucket, Key: key, Body: "x" }))
    }

    const root = await adminJson<{
      prefixes: string[]
      objects: { key: string }[]
    }>(`/_admin/buckets/${bucket}/objects`)

    expect(root.prefixes).toEqual(["photos/"])
    expect(root.objects.map((object) => object.key)).toEqual(["notes.txt"])

    const inside = await adminJson<{ objects: { key: string }[] }>(
      `/_admin/buckets/${bucket}/objects?prefix=photos/`,
    )
    expect(inside.objects.map((object) => object.key)).toEqual([
      "photos/a.jpg",
      "photos/b.jpg",
    ])
  })

  it("should list every key when the delimiter is cleared", async () => {
    const bucket = await createBucket("admin-flat")
    for (const key of ["photos/a.jpg", "notes.txt"]) {
      await s3.send(new PutObjectCommand({ Bucket: bucket, Key: key, Body: "x" }))
    }

    const flat = await adminJson<{ prefixes: string[]; objects: { key: string }[] }>(
      `/_admin/buckets/${bucket}/objects?delimiter=`,
    )

    expect(flat.prefixes).toEqual([])
    expect(flat.objects.map((object) => object.key)).toEqual(["notes.txt", "photos/a.jpg"])
  })
})

describe("the object endpoint", () => {
  it("should upload, describe and delete an object", async () => {
    const bucket = await createBucket("admin-object")
    const key = "folder/report.txt"

    const uploaded = await admin(
      `/_admin/buckets/${bucket}/object?key=${encodeURIComponent(key)}`,
      {
        method: "PUT",
        headers: { "content-type": "text/plain" },
        body: "hello from the console",
      },
    )
    expect(uploaded.status).toBe(201)

    const detail = await adminJson<{
      key: string
      size: number
      content_type: string
      versions: unknown[]
    }>(`/_admin/buckets/${bucket}/object?key=${encodeURIComponent(key)}`)

    expect(detail.key).toBe(key)
    expect(detail.size).toBe(22)
    expect(detail.content_type).toBe("text/plain")

    const removed = await admin(
      `/_admin/buckets/${bucket}/object?key=${encodeURIComponent(key)}`,
      { method: "DELETE" },
    )
    expect(removed.status).toBe(204)
  })

  // A key is an arbitrary string, which is why it travels as a query parameter rather
  // than a path segment.
  it("should handle a key containing characters a path could not carry", async () => {
    const bucket = await createBucket("admin-oddkey")
    const key = "a b/c+d&e.txt"

    await admin(`/_admin/buckets/${bucket}/object?key=${encodeURIComponent(key)}`, {
      method: "PUT",
      body: "odd",
    })

    const detail = await adminJson<{ key: string }>(
      `/_admin/buckets/${bucket}/object?key=${encodeURIComponent(key)}`,
    )
    expect(detail.key).toBe(key)
  })

  it("should report a key that is not there", async () => {
    const bucket = await createBucket("admin-nokey")

    const response = await admin(`/_admin/buckets/${bucket}/object?key=absent.txt`)

    expect(response.status).toBe(404)
    expect(((await response.json()) as { error: string }).error).toBe("NoSuchKey")
  })
})

describe("GET /_admin/buckets/{bucket}/content", () => {
  const content = (bucket: string, key: string, download = false) =>
    admin(
      `/_admin/buckets/${bucket}/content?key=${encodeURIComponent(key)}${download ? "&download=1" : ""}`,
    )

  it("should serve the bytes with the object's own type", async () => {
    const bucket = await createBucket("admin-content")
    await s3.send(
      new PutObjectCommand({
        Bucket: bucket,
        Key: "note.txt",
        Body: "hello",
        ContentType: "text/plain",
      }),
    )

    const response = await content(bucket, "note.txt")

    expect(response.status).toBe(200)
    expect(response.headers.get("content-type")).toContain("text/plain")
    expect(await response.text()).toBe("hello")
  })

  // A client can store anything under any content type. Serving an uploaded page as
  // text/html from the console's own origin would let it script the admin API, so the
  // type is neutered and sniffing is turned off.
  it("should refuse to serve stored html as html", async () => {
    const bucket = await createBucket("admin-html")
    await s3.send(
      new PutObjectCommand({
        Bucket: bucket,
        Key: "evil.html",
        Body: "<script>fetch('/_admin/reset', { method: 'POST' })</script>",
        ContentType: "text/html",
      }),
    )

    const response = await content(bucket, "evil.html")

    expect(response.headers.get("content-type")).toContain("text/plain")
    expect(response.headers.get("content-type")).not.toContain("html")
    expect(response.headers.get("x-content-type-options")).toBe("nosniff")
  })

  it("should neuter svg and javascript the same way", async () => {
    const bucket = await createBucket("admin-svg")
    for (const [key, type] of [
      ["a.svg", "image/svg+xml"],
      ["b.js", "text/javascript"],
    ]) {
      await s3.send(
        new PutObjectCommand({ Bucket: bucket, Key: key, Body: "x", ContentType: type }),
      )
      const response = await content(bucket, key)
      expect(response.headers.get("content-type")).toContain("text/plain")
    }
  })

  it("should offer a download when asked", async () => {
    const bucket = await createBucket("admin-dl")
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "deep/report.txt", Body: "x" }))

    const inline = await content(bucket, "deep/report.txt")
    const attached = await content(bucket, "deep/report.txt", true)

    expect(inline.headers.get("content-disposition")).toBe("inline")
    expect(attached.headers.get("content-disposition")).toContain("attachment")
    expect(attached.headers.get("content-disposition")).toContain("report.txt")
  })

  it("should report a key that is not there", async () => {
    const bucket = await createBucket("admin-nocontent")

    const response = await content(bucket, "absent.bin")

    expect(response.status).toBe(404)
  })
})

describe("POST /_admin/reset/{bucket}", () => {
  it("should empty a bucket and keep it", async () => {
    const bucket = await createBucket("admin-purge")
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "a.txt", Body: "x" }))

    const report = await adminJson<{ reset: string; objects_removed: number }>(
      `/_admin/reset/${bucket}`,
      { method: "POST" },
    )

    expect(report.objects_removed).toBe(1)
    const detail = await adminJson<{ objects: number }>(`/_admin/buckets/${bucket}`)
    expect(detail.objects).toBe(0)
  })
})
