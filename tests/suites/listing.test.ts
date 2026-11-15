import { ListObjectsV2Command, PutObjectCommand } from "@aws-sdk/client-s3";
import { beforeAll, describe, expect, it } from "vitest";

import { createBucket, s3 } from "../support/s3";

let bucket: string;

const keys = [
  "a.txt",
  "b.txt",
  "photos/2024/one.jpg",
  "photos/2024/two.jpg",
  "photos/cover.png",
  "videos/clip.mp4",
];

beforeAll(async () => {
  bucket = await createBucket("listing");
  for (const Key of keys) {
    await s3.send(new PutObjectCommand({ Bucket: bucket, Key, Body: "x" }));
  }
});

describe("ListObjectsV2", () => {
  it("should return every key when given no prefix or delimiter", async () => {
    const listed = await s3.send(new ListObjectsV2Command({ Bucket: bucket }));

    expect(listed.Contents?.map((entry) => entry.Key)).toEqual([...keys].sort());
    expect(listed.KeyCount).toBe(keys.length);
    expect(listed.IsTruncated).toBe(false);
  });

  it("should roll keys up into common prefixes when given a delimiter", async () => {
    const listed = await s3.send(
      new ListObjectsV2Command({ Bucket: bucket, Delimiter: "/" }),
    );

    expect(listed.CommonPrefixes?.map((entry) => entry.Prefix)).toEqual([
      "photos/",
      "videos/",
    ]);
    expect(listed.Contents?.map((entry) => entry.Key)).toEqual(["a.txt", "b.txt"]);
  });

  it("should descend one level when given a prefix and a delimiter", async () => {
    const listed = await s3.send(
      new ListObjectsV2Command({ Bucket: bucket, Prefix: "photos/", Delimiter: "/" }),
    );

    expect(listed.CommonPrefixes?.map((entry) => entry.Prefix)).toEqual(["photos/2024/"]);
    expect(listed.Contents?.map((entry) => entry.Key)).toEqual(["photos/cover.png"]);
  });

  it("should return nothing for a prefix that matches no key", async () => {
    const listed = await s3.send(
      new ListObjectsV2Command({ Bucket: bucket, Prefix: "nothing-here/" }),
    );

    expect(listed.KeyCount).toBe(0);
    expect(listed.Contents ?? []).toEqual([]);
  });

  it("should page through keys with a continuation token", async () => {
    const first = await s3.send(new ListObjectsV2Command({ Bucket: bucket, MaxKeys: 2 }));

    expect(first.IsTruncated).toBe(true);
    expect(first.Contents).toHaveLength(2);
    expect(first.NextContinuationToken).toBeDefined();

    const second = await s3.send(
      new ListObjectsV2Command({
        Bucket: bucket,
        MaxKeys: 2,
        ContinuationToken: first.NextContinuationToken,
      }),
    );

    const firstKeys = first.Contents?.map((entry) => entry.Key) ?? [];
    const secondKeys = second.Contents?.map((entry) => entry.Key) ?? [];
    expect(secondKeys).toHaveLength(2);
    expect(secondKeys.some((key) => firstKeys.includes(key))).toBe(false);
  });

  it("should walk the whole bucket across pages without repeating a key", async () => {
    const seen: string[] = [];
    let token: string | undefined;

    do {
      const page = await s3.send(
        new ListObjectsV2Command({ Bucket: bucket, MaxKeys: 2, ContinuationToken: token }),
      );
      seen.push(...(page.Contents?.map((entry) => entry.Key ?? "") ?? []));
      token = page.IsTruncated ? page.NextContinuationToken : undefined;
    } while (token);

    expect(seen).toEqual([...keys].sort());
  });

  it("should report sizes and ETags for listed objects", async () => {
    const listed = await s3.send(new ListObjectsV2Command({ Bucket: bucket, MaxKeys: 1 }));
    const entry = listed.Contents?.[0];

    expect(entry?.Size).toBe(1);
    expect(entry?.ETag).toMatch(/^"[0-9a-f]{32}"$/);
    expect(entry?.LastModified).toBeInstanceOf(Date);
  });
});
