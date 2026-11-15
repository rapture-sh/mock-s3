import {
  CopyObjectCommand,
  DeleteObjectCommand,
  DeleteObjectsCommand,
  GetBucketVersioningCommand,
  GetObjectCommand,
  HeadObjectCommand,
  ListObjectsV2Command,
  ListObjectVersionsCommand,
  PutBucketVersioningCommand,
  PutObjectCommand,
} from "@aws-sdk/client-s3";
import { beforeAll, describe, expect, it } from "vitest";

import { collect, createBucket, errorCode, s3 } from "../support/s3";

async function setVersioning(bucket: string, status: "Enabled" | "Suspended"): Promise<void> {
  await s3.send(
    new PutBucketVersioningCommand({ Bucket: bucket, VersioningConfiguration: { Status: status } }),
  );
}

/** A bucket with versioning already enabled. */
async function versionedBucket(prefix: string): Promise<string> {
  const bucket = await createBucket(prefix);
  await setVersioning(bucket, "Enabled");
  return bucket;
}

async function put(bucket: string, key: string, body: string): Promise<string | undefined> {
  const written = await s3.send(new PutObjectCommand({ Bucket: bucket, Key: key, Body: body }));
  return written.VersionId;
}

async function bodyOf(bucket: string, key: string, versionId?: string): Promise<string> {
  const got = await s3.send(new GetObjectCommand({ Bucket: bucket, Key: key, VersionId: versionId }));
  return (await collect(got.Body)).toString();
}

describe("GetBucketVersioning", () => {
  it("should report no status before versioning is configured", async () => {
    const bucket = await createBucket("unversioned");

    const got = await s3.send(new GetBucketVersioningCommand({ Bucket: bucket }));

    expect(got.Status).toBeUndefined();
  });

  it("should report each status it was set to", async () => {
    const bucket = await versionedBucket("status");

    expect((await s3.send(new GetBucketVersioningCommand({ Bucket: bucket }))).Status).toBe(
      "Enabled",
    );

    await setVersioning(bucket, "Suspended");
    expect((await s3.send(new GetBucketVersioningCommand({ Bucket: bucket }))).Status).toBe(
      "Suspended",
    );
  });

  it("should reject a configuration with no status", async () => {
    const bucket = await createBucket("illegal");

    const code = await errorCode(() =>
      s3.send(
        new PutBucketVersioningCommand({ Bucket: bucket, VersioningConfiguration: {} }),
      ),
    );

    expect(code).toBe("IllegalVersioningConfigurationException");
  });
});

describe("PutObject", () => {
  it("should not report a version in an unversioned bucket", async () => {
    const bucket = await createBucket("plain");

    expect(await put(bucket, "notes.txt", "one")).toBeUndefined();
  });

  it("should keep every write once versioning is enabled", async () => {
    const bucket = await versionedBucket("kept");

    const first = await put(bucket, "notes.txt", "one");
    const second = await put(bucket, "notes.txt", "two");

    expect(first).not.toBe(second);
    expect(await bodyOf(bucket, "notes.txt")).toBe("two");
    expect(await bodyOf(bucket, "notes.txt", first)).toBe("one");
  });

  // Enabling versioning on a bucket that already holds objects must not lose them: they
  // become the null version, exactly as on real S3.
  it("should keep an object written before versioning was enabled", async () => {
    const bucket = await createBucket("existing");
    await put(bucket, "notes.txt", "before");

    await setVersioning(bucket, "Enabled");

    expect(await bodyOf(bucket, "notes.txt", "null")).toBe("before");
    expect(await bodyOf(bucket, "notes.txt")).toBe("before");
  });
});

describe("GetObject", () => {
  it("should report the version it served", async () => {
    const bucket = await versionedBucket("served");
    const written = await put(bucket, "notes.txt", "one");

    const got = await s3.send(new GetObjectCommand({ Bucket: bucket, Key: "notes.txt" }));

    expect(got.VersionId).toBe(written);
  });

  it("should report NoSuchVersion for an id that names nothing", async () => {
    const bucket = await versionedBucket("absent");
    await put(bucket, "notes.txt", "one");

    const code = await errorCode(() =>
      s3.send(
        new GetObjectCommand({ Bucket: bucket, Key: "notes.txt", VersionId: "deadbeef" }),
      ),
    );

    expect(code).toBe("NoSuchVersion");
  });
});

describe("HeadObject", () => {
  it("should describe the version its id names", async () => {
    const bucket = await versionedBucket("head");
    const first = await put(bucket, "notes.txt", "one");
    await put(bucket, "notes.txt", "two but longer");

    const older = await s3.send(
      new HeadObjectCommand({ Bucket: bucket, Key: "notes.txt", VersionId: first }),
    );
    const current = await s3.send(new HeadObjectCommand({ Bucket: bucket, Key: "notes.txt" }));

    expect(older.VersionId).toBe(first);
    expect(older.ContentLength).toBe(3);
    expect(current.ContentLength).toBe(14);
  });
});

describe("DeleteObject", () => {
  it("should leave a delete marker rather than remove the object", async () => {
    const bucket = await versionedBucket("marked");
    const written = await put(bucket, "notes.txt", "one");

    const deleted = await s3.send(new DeleteObjectCommand({ Bucket: bucket, Key: "notes.txt" }));

    expect(deleted.DeleteMarker).toBe(true);
    expect(deleted.VersionId).not.toBe(written);
    expect(await bodyOf(bucket, "notes.txt", written)).toBe("one");
  });

  it("should make the key read as absent once a marker is current", async () => {
    const bucket = await versionedBucket("hidden");
    await put(bucket, "notes.txt", "one");
    await s3.send(new DeleteObjectCommand({ Bucket: bucket, Key: "notes.txt" }));

    const code = await errorCode(() =>
      s3.send(new GetObjectCommand({ Bucket: bucket, Key: "notes.txt" })),
    );

    expect(code).toBe("NoSuchKey");
  });

  it("should restore the object when its delete marker is removed", async () => {
    const bucket = await versionedBucket("restored");
    await put(bucket, "notes.txt", "one");
    const marker = await s3.send(new DeleteObjectCommand({ Bucket: bucket, Key: "notes.txt" }));

    const removed = await s3.send(
      new DeleteObjectCommand({ Bucket: bucket, Key: "notes.txt", VersionId: marker.VersionId }),
    );

    expect(removed.DeleteMarker).toBe(true);
    expect(await bodyOf(bucket, "notes.txt")).toBe("one");
  });

  it("should remove one version outright when its id is named", async () => {
    const bucket = await versionedBucket("pruned");
    const first = await put(bucket, "notes.txt", "one");
    await put(bucket, "notes.txt", "two");

    await s3.send(
      new DeleteObjectCommand({ Bucket: bucket, Key: "notes.txt", VersionId: first }),
    );

    expect(await bodyOf(bucket, "notes.txt")).toBe("two");
    const code = await errorCode(() =>
      s3.send(new GetObjectCommand({ Bucket: bucket, Key: "notes.txt", VersionId: first })),
    );
    expect(code).toBe("NoSuchVersion");
  });
});

describe("DeleteObjects", () => {
  it("should report the delete marker it created for each key", async () => {
    const bucket = await versionedBucket("batch");
    await put(bucket, "one.txt", "a");
    await put(bucket, "two.txt", "b");

    const result = await s3.send(
      new DeleteObjectsCommand({
        Bucket: bucket,
        Delete: { Objects: [{ Key: "one.txt" }, { Key: "two.txt" }] },
      }),
    );

    expect(result.Deleted).toHaveLength(2);
    for (const entry of result.Deleted ?? []) {
      expect(entry.DeleteMarker).toBe(true);
      expect(entry.DeleteMarkerVersionId).toBeDefined();
    }
  });

  // Enumerating ?versions and deleting each entry is how a client empties a versioned
  // bucket, so the two have to agree on what is in it.
  it("should empty a versioned bucket when every version is named", async () => {
    const bucket = await versionedBucket("emptied");
    await put(bucket, "notes.txt", "one");
    await put(bucket, "notes.txt", "two");
    await s3.send(new DeleteObjectCommand({ Bucket: bucket, Key: "notes.txt" }));

    const listed = await s3.send(new ListObjectVersionsCommand({ Bucket: bucket }));
    const targets = [...(listed.Versions ?? []), ...(listed.DeleteMarkers ?? [])].map((entry) => ({
      Key: entry.Key,
      VersionId: entry.VersionId,
    }));
    expect(targets).toHaveLength(3);

    const result = await s3.send(
      new DeleteObjectsCommand({ Bucket: bucket, Delete: { Objects: targets } }),
    );

    expect(result.Errors ?? []).toHaveLength(0);
    const after = await s3.send(new ListObjectVersionsCommand({ Bucket: bucket }));
    expect(after.Versions ?? []).toHaveLength(0);
    expect(after.DeleteMarkers ?? []).toHaveLength(0);
  });
});

describe("ListObjectVersions", () => {
  it("should report every version newest first", async () => {
    const bucket = await versionedBucket("listed");
    const first = await put(bucket, "notes.txt", "one");
    const second = await put(bucket, "notes.txt", "two");

    const listed = await s3.send(new ListObjectVersionsCommand({ Bucket: bucket }));

    expect(listed.Versions?.map((version) => version.VersionId)).toEqual([second, first]);
    expect(listed.Versions?.[0].IsLatest).toBe(true);
    expect(listed.Versions?.[1].IsLatest).toBe(false);
  });

  it("should report a delete marker separately from a version", async () => {
    const bucket = await versionedBucket("markers");
    await put(bucket, "notes.txt", "one");
    await s3.send(new DeleteObjectCommand({ Bucket: bucket, Key: "notes.txt" }));

    const listed = await s3.send(new ListObjectVersionsCommand({ Bucket: bucket }));

    expect(listed.Versions).toHaveLength(1);
    expect(listed.DeleteMarkers).toHaveLength(1);
    expect(listed.DeleteMarkers?.[0].IsLatest).toBe(true);
  });

  // A key can hold more versions than one page can carry, so resuming has to land inside
  // the key rather than skipping the rest of it.
  it("should paginate within a single key", async () => {
    const bucket = await versionedBucket("paged");
    const written: string[] = [];
    for (const body of ["one", "two", "three"]) {
      written.push((await put(bucket, "notes.txt", body)) as string);
    }
    written.reverse();

    const first = await s3.send(
      new ListObjectVersionsCommand({ Bucket: bucket, MaxKeys: 2 }),
    );
    expect(first.IsTruncated).toBe(true);
    expect(first.NextKeyMarker).toBe("notes.txt");

    const second = await s3.send(
      new ListObjectVersionsCommand({
        Bucket: bucket,
        MaxKeys: 2,
        KeyMarker: first.NextKeyMarker,
        VersionIdMarker: first.NextVersionIdMarker,
      }),
    );

    expect(second.IsTruncated).toBe(false);
    const seen = [...(first.Versions ?? []), ...(second.Versions ?? [])].map(
      (version) => version.VersionId,
    );
    expect(seen).toEqual(written);
  });

  it("should roll up versions under a delimiter", async () => {
    const bucket = await versionedBucket("rolled");
    await put(bucket, "photos/one.jpg", "one");
    await put(bucket, "photos/one.jpg", "again");
    await put(bucket, "notes.txt", "one");

    const listed = await s3.send(
      new ListObjectVersionsCommand({ Bucket: bucket, Delimiter: "/" }),
    );

    expect(listed.CommonPrefixes?.map((entry) => entry.Prefix)).toEqual(["photos/"]);
    expect(listed.Versions?.map((version) => version.Key)).toEqual(["notes.txt"]);
  });

  // The listing a client enumerates before deleting a bucket, whatever its versioning is.
  it("should report a null version for each object in an unversioned bucket", async () => {
    const bucket = await createBucket("nulls");
    await put(bucket, "notes.txt", "one");

    const listed = await s3.send(new ListObjectVersionsCommand({ Bucket: bucket }));

    expect(listed.Versions).toHaveLength(1);
    expect(listed.Versions?.[0].VersionId).toBe("null");
    expect(listed.Versions?.[0].IsLatest).toBe(true);
  });
});

describe("ListObjectsV2", () => {
  it("should show only the current version and skip a deleted key", async () => {
    const bucket = await versionedBucket("current");
    await put(bucket, "notes.txt", "one");
    await put(bucket, "notes.txt", "two");
    await put(bucket, "other.txt", "one");
    await s3.send(new DeleteObjectCommand({ Bucket: bucket, Key: "other.txt" }));

    const listed = await s3.send(new ListObjectsV2Command({ Bucket: bucket }));

    expect(listed.Contents?.map((object) => object.Key)).toEqual(["notes.txt"]);
    expect(listed.Contents?.[0].Size).toBe(3);
  });
});

describe("CopyObject", () => {
  let bucket: string;
  let first: string | undefined;

  beforeAll(async () => {
    bucket = await versionedBucket("copied");
    first = await put(bucket, "notes.txt", "one");
    await put(bucket, "notes.txt", "two");
  });

  it("should copy the version the source names", async () => {
    const copied = await s3.send(
      new CopyObjectCommand({
        Bucket: bucket,
        Key: "from-version.txt",
        CopySource: `${bucket}/notes.txt?versionId=${first}`,
      }),
    );

    expect(copied.CopySourceVersionId).toBe(first);
    expect(copied.VersionId).toBeDefined();
    expect(await bodyOf(bucket, "from-version.txt")).toBe("one");
  });

  it("should copy the current version when the source names none", async () => {
    await s3.send(
      new CopyObjectCommand({
        Bucket: bucket,
        Key: "from-current.txt",
        CopySource: `${bucket}/notes.txt`,
      }),
    );

    expect(await bodyOf(bucket, "from-current.txt")).toBe("two");
  });
});
