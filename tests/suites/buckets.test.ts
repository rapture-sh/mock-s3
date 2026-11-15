import {
  CreateBucketCommand,
  DeleteBucketCommand,
  HeadBucketCommand,
  ListBucketsCommand,
  PutObjectCommand,
} from "@aws-sdk/client-s3";
import { describe, expect, it } from "vitest";

import { createBucket, errorCode, s3, uniqueBucket } from "../support/s3";

describe("CreateBucket", () => {
  it("should create a bucket and then list it", async () => {
    const bucket = await createBucket();

    const listed = await s3.send(new ListBucketsCommand({}));

    expect(listed.Buckets?.map((entry) => entry.Name)).toContain(bucket);
  });

  it("should report BucketAlreadyOwnedByYou when the bucket already exists", async () => {
    const bucket = await createBucket();

    const code = await errorCode(() => s3.send(new CreateBucketCommand({ Bucket: bucket })));

    expect(code).toBe("BucketAlreadyOwnedByYou");
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
