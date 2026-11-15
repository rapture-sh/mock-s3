import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // Every spec talks to a server over the network, so the default 5s is tight once
    // a multipart upload is involved.
    testTimeout: 30_000,
    hookTimeout: 30_000,
    // Each test uses its own bucket, so specs are safe to run together.
    fileParallelism: true,
    include: ["suites/**/*.test.ts"],
  },
});
