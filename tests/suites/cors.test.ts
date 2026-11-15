import {
  DeleteBucketCorsCommand,
  PutBucketCorsCommand,
  PutObjectCommand,
} from "@aws-sdk/client-s3";
import { beforeAll, describe, expect, it } from "vitest";

import { createBucket, endpoint, s3 } from "../support/s3";

const origin = "https://app.example.com";

let bucket: string;

beforeAll(async () => {
  bucket = await createBucket("cors");
  await s3.send(new PutObjectCommand({ Bucket: bucket, Key: "file.txt", Body: "payload" }));
});

/** A preflight, exactly as a browser sends one. */
function preflight(
  path: string,
  init: { origin?: string; method?: string; headers?: string } = {},
) {
  const headers: Record<string, string> = {
    Origin: init.origin ?? origin,
    "Access-Control-Request-Method": init.method ?? "PUT",
  };
  if (init.headers) headers["Access-Control-Request-Headers"] = init.headers;

  return fetch(`${endpoint}${path}`, { method: "OPTIONS", headers });
}

describe("CORS preflight", () => {
  it("should refuse a preflight before any configuration exists", async () => {
    const fresh = await createBucket("cors-unset");

    const response = await preflight(`/${fresh}/file.txt`);

    // S3 refuses rather than allowing by default, so a web app that would fail against
    // AWS fails here too.
    expect(response.status).toBe(403);
    expect(response.headers.get("access-control-allow-origin")).toBeNull();
  });

  it("should allow a configured origin and method", async () => {
    await s3.send(
      new PutBucketCorsCommand({
        Bucket: bucket,
        CORSConfiguration: {
          CORSRules: [
            {
              AllowedOrigins: [origin],
              AllowedMethods: ["GET", "PUT"],
              AllowedHeaders: ["*"],
              ExposeHeaders: ["ETag"],
              MaxAgeSeconds: 3000,
            },
          ],
        },
      }),
    );

    const response = await preflight(`/${bucket}/file.txt`, {
      headers: "content-type, x-amz-acl",
    });

    expect(response.status).toBe(200);
    expect(response.headers.get("access-control-allow-origin")).toBe(origin);
    expect(response.headers.get("access-control-allow-methods")).toBe("GET, PUT");
    expect(response.headers.get("access-control-allow-headers")).toBe(
      "content-type, x-amz-acl",
    );
    expect(response.headers.get("access-control-expose-headers")).toBe("ETag");
    expect(response.headers.get("access-control-max-age")).toBe("3000");
    expect(response.headers.get("vary")).toBe("Origin");
  });

  it("should refuse an origin no rule covers", async () => {
    const response = await preflight(`/${bucket}/file.txt`, {
      origin: "https://evil.example.com",
    });

    expect(response.status).toBe(403);
  });

  it("should refuse a method the rule does not list", async () => {
    const response = await preflight(`/${bucket}/file.txt`, { method: "DELETE" });

    expect(response.status).toBe(403);
  });

  it("should put the allow-origin header on the real request too", async () => {
    // A browser checks the actual response as well as the preflight; without this the
    // fetch fails after a preflight that succeeded.
    const response = await fetch(`${endpoint}/${bucket}/file.txt`, {
      headers: { Origin: origin },
    });

    expect(response.status).toBe(200);
    expect(response.headers.get("access-control-allow-origin")).toBe(origin);
    expect(await response.text()).toBe("payload");
  });

  it("should leave an ordinary request without CORS headers", async () => {
    const response = await fetch(`${endpoint}/${bucket}/file.txt`);

    expect(response.status).toBe(200);
    expect(response.headers.get("access-control-allow-origin")).toBeNull();
  });

  it("should support a wildcard subdomain pattern", async () => {
    const wild = await createBucket("cors-wild");
    await s3.send(
      new PutBucketCorsCommand({
        Bucket: wild,
        CORSConfiguration: {
          CORSRules: [
            { AllowedOrigins: ["https://*.example.com"], AllowedMethods: ["GET"] },
          ],
        },
      }),
    );

    const allowed = await preflight(`/${wild}/anything`, {
      origin: "https://deep.example.com",
      method: "GET",
    });
    const refused = await preflight(`/${wild}/anything`, {
      origin: "https://example.org",
      method: "GET",
    });

    expect(allowed.status).toBe(200);
    expect(refused.status).toBe(403);
  });

  it("should refuse again once the configuration is deleted", async () => {
    const temporary = await createBucket("cors-temp");
    await s3.send(
      new PutBucketCorsCommand({
        Bucket: temporary,
        CORSConfiguration: {
          CORSRules: [{ AllowedOrigins: ["*"], AllowedMethods: ["GET"] }],
        },
      }),
    );
    expect((await preflight(`/${temporary}/x`, { method: "GET" })).status).toBe(200);

    await s3.send(new DeleteBucketCorsCommand({ Bucket: temporary }));

    expect((await preflight(`/${temporary}/x`, { method: "GET" })).status).toBe(403);
  });
});
