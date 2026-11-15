import { randomUUID } from "node:crypto";

import { CreateBucketCommand, S3Client } from "@aws-sdk/client-s3";

/**
 * Where the server under test is. Defaults to a mock-s3 started on the host; the compose
 * target sets it to the container.
 */
export const endpoint = process.env.S3_ENDPOINT ?? "http://localhost:9000";

/** The console and admin API, for tests that assert on the request log or reset state. */
export const consoleEndpoint = process.env.S3_CONSOLE_ENDPOINT ?? "http://localhost:9001";

/**
 * A client configured the way a real caller has to configure one against mock-s3.
 *
 * `forcePathStyle` is the part people forget: without it the SDK addresses
 * `bucket.localhost`, which has no DNS entry.
 */
export function makeClient(): S3Client {
  return new S3Client({
    endpoint,
    region: process.env.S3_REGION ?? "us-east-1",
    forcePathStyle: true,
    credentials: { accessKeyId: "test", secretAccessKey: "test" },
  });
}

export const s3 = makeClient();

/**
 * A bucket name unique to one test. Giving every test its own bucket is the isolation
 * mechanism, so specs can run in parallel against one shared server.
 */
export function uniqueBucket(prefix = "test"): string {
  return `${prefix}-${randomUUID()}`;
}

/** Create a fresh bucket and return its name. */
export async function createBucket(prefix?: string): Promise<string> {
  const bucket = uniqueBucket(prefix);
  await s3.send(new CreateBucketCommand({ Bucket: bucket }));
  return bucket;
}

/** Read a response body stream into a Buffer. */
export async function collect(body: unknown): Promise<Buffer> {
  const stream = body as AsyncIterable<Uint8Array>;
  const chunks: Uint8Array[] = [];
  for await (const chunk of stream) {
    chunks.push(chunk);
  }
  return Buffer.concat(chunks);
}

/**
 * Run `operation` and return the S3 error code it failed with.
 *
 * Asserting on the code rather than the message is deliberate: the code is the contract
 * clients branch on, the wording is not.
 */
export async function errorCode(operation: () => Promise<unknown>): Promise<string> {
  try {
    await operation();
  } catch (error) {
    const named = error as { name?: string; Code?: string };
    return named.Code ?? named.name ?? "unknown";
  }
  throw new Error("expected the operation to fail, but it succeeded");
}
