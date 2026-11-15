import { randomBytes, createHash } from "node:crypto";

import {
  DeleteObjectCommand,
  GetObjectCommand,
  HeadObjectCommand,
  PutObjectCommand,
} from "@aws-sdk/client-s3";
import { beforeAll, describe, expect, it } from "vitest";

import { collect, createBucket, errorCode, s3 } from "../support/s3";

let bucket: string;

beforeAll(async () => {
  bucket = await createBucket("objects");
});

describe("PutObject and GetObject", () => {
  it("should round-trip text unchanged", async () => {
    await s3.send(
      new PutObjectCommand({
        Bucket: bucket,
        Key: "text",
        Body: "hello from mock-s3",
        ContentType: "text/plain",
      }),
    );

    const got = await s3.send(new GetObjectCommand({ Bucket: bucket, Key: "text" }));

    expect(got.ContentType).toBe("text/plain");
    expect((await collect(got.Body)).toString()).toBe("hello from mock-s3");
  });

  it("should round-trip binary content byte for byte", async () => {
    const payload = randomBytes(64 * 1024);

    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "binary", Body: payload }));
    const got = await s3.send(new GetObjectCommand({ Bucket: bucket, Key: "binary" }));

    expect(Buffer.compare(await collect(got.Body), payload)).toBe(0);
  });

  it("should report an ETag that is the quoted MD5 of the payload", async () => {
    const payload = randomBytes(1024);
    const expected = createHash("md5").update(payload).digest("hex");

    const put = await s3.send(
      new PutObjectCommand({ Bucket: bucket, Key: "etag", Body: payload }),
    );

    expect(put.ETag).toBe(`"${expected}"`);
  });

  it("should round-trip user metadata", async () => {
    await s3.send(
      new PutObjectCommand({
        Bucket: bucket,
        Key: "with-metadata",
        Body: "x",
        Metadata: { owner: "abi", purpose: "test" },
      }),
    );

    const head = await s3.send(new HeadObjectCommand({ Bucket: bucket, Key: "with-metadata" }));

    expect(head.Metadata).toMatchObject({ owner: "abi", purpose: "test" });
  });

  it("should store an empty object", async () => {
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "empty", Body: "" }));

    const head = await s3.send(new HeadObjectCommand({ Bucket: bucket, Key: "empty" }));

    expect(head.ContentLength).toBe(0);
  });

  it.each([
    ["a key with spaces.txt", "spaces"],
    ["café/über/日本語.txt", "unicode"],
    ["notes&misc=1+2.txt", "url syntax"],
    ["deeply/nested/path/to/a/file.bin", "nesting"],
  ])("should handle %s (%s)", async (key) => {
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: key, Body: "payload" }));

    const got = await s3.send(new GetObjectCommand({ Bucket: bucket, Key: key }));

    expect((await collect(got.Body)).toString()).toBe("payload");
  });
});

describe("HeadObject", () => {
  it("should report the object size", async () => {
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "sized", Body: "0123456789" }));

    const head = await s3.send(new HeadObjectCommand({ Bucket: bucket, Key: "sized" }));

    expect(head.ContentLength).toBe(10);
  });

  it("should fail for a key that does not exist", async () => {
    const code = await errorCode(() =>
      s3.send(new HeadObjectCommand({ Bucket: bucket, Key: "absent" })),
    );

    expect(code).not.toBe("unknown");
  });
});

describe("GetObject with a Range", () => {
  beforeAll(async () => {
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "ranged", Body: "0123456789" }));
  });

  it("should return a closed range", async () => {
    const got = await s3.send(
      new GetObjectCommand({ Bucket: bucket, Key: "ranged", Range: "bytes=2-4" }),
    );

    expect((await collect(got.Body)).toString()).toBe("234");
    expect(got.ContentRange).toBe("bytes 2-4/10");
  });

  it("should return an open-ended range", async () => {
    const got = await s3.send(
      new GetObjectCommand({ Bucket: bucket, Key: "ranged", Range: "bytes=7-" }),
    );

    expect((await collect(got.Body)).toString()).toBe("789");
  });

  it("should return a suffix range", async () => {
    const got = await s3.send(
      new GetObjectCommand({ Bucket: bucket, Key: "ranged", Range: "bytes=-3" }),
    );

    expect((await collect(got.Body)).toString()).toBe("789");
  });
});

describe("DeleteObject", () => {
  it("should remove an object", async () => {
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "doomed", Body: "x" }));

    await s3.send(new DeleteObjectCommand({ Bucket: bucket, Key: "doomed" }));

    const code = await errorCode(() =>
      s3.send(new GetObjectCommand({ Bucket: bucket, Key: "doomed" })),
    );
    expect(code).toBe("NoSuchKey");
  });

  it("should succeed for a key that was never there", async () => {
    await expect(
      s3.send(new DeleteObjectCommand({ Bucket: bucket, Key: "never-existed" })),
    ).resolves.toBeDefined();
  });
});
