import {
  CreateBucketCommand,
  DeleteBucketCommand,
  HeadBucketCommand,
  ListBucketsCommand,
  PutObjectCommand,
} from "@aws-sdk/client-s3";
import { describe, expect, it } from "vitest";

import { createBucket, endpoint, errorCode, s3, uniqueBucket } from "../support/s3";

describe("an unhandled method", () => {
  it("should answer with an S3 error rather than an empty body", async () => {
    const bucket = await createBucket("method");

    // A bare 405 with no body makes a client report "invalid XML received", which says
    // nothing about what went wrong.
    const response = await fetch(`${endpoint}/${bucket}`, { method: "PATCH" });
    const body = await response.text();

    expect(response.status).toBe(501);
    expect(response.headers.get("content-type")).toBe("application/xml");
    expect(body).toContain("<Code>NotImplemented</Code>");
    expect(body).toContain("PATCH");
  });
});

describe("CreateBucket", () => {
  it("should create a bucket and then list it", async () => {
    const bucket = await createBucket();

    const listed = await s3.send(new ListBucketsCommand({}));

    expect(listed.Buckets?.map((entry) => entry.Name)).toContain(bucket);
  });

  // Re-creating a bucket you already own succeeds in us-east-1 and fails with
  // BucketAlreadyOwnedByYou everywhere else. us-east-1 is the region mock-s3 reports, so
  // setup code that creates a bucket unconditionally has to work.
  it("should succeed when the bucket already exists, as us-east-1 does", async () => {
    const bucket = await createBucket();

    await expect(s3.send(new CreateBucketCommand({ Bucket: bucket }))).resolves.toBeDefined();
  });

  it("should reject a name that real S3 would reject", async () => {
    const code = await errorCode(() =>
      s3.send(new CreateBucketCommand({ Bucket: "Not-A-Valid-Name" })),
    );

    expect(code).toBe("InvalidBucketName");
  });
});

describe("HeadBucket", () => {
  it("should succeed for a bucket that exists", async () => {
    const bucket = await createBucket();

    await expect(s3.send(new HeadBucketCommand({ Bucket: bucket }))).resolves.toBeDefined();
  });

  it("should fail for a bucket that does not exist", async () => {
    const code = await errorCode(() =>
      s3.send(new HeadBucketCommand({ Bucket: uniqueBucket("absent") })),
    );

    expect(code).not.toBe("unknown");
  });
});

describe("DeleteBucket", () => {
  it("should delete an empty bucket", async () => {
    const bucket = await createBucket();

    await s3.send(new DeleteBucketCommand({ Bucket: bucket }));

    const code = await errorCode(() => s3.send(new HeadBucketCommand({ Bucket: bucket })));
    expect(code).not.toBe("unknown");
  });

  it("should refuse to delete a bucket that still holds objects", async () => {
    const bucket = await createBucket();
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "a", Body: "x" }));

    const code = await errorCode(() => s3.send(new DeleteBucketCommand({ Bucket: bucket })));

    expect(code).toBe("BucketNotEmpty");
  });
});
