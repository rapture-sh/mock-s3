import { resolve } from "node:path"
import tailwindcss from "@tailwindcss/vite"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"

/**
 * The console is served by the mock-s3 binary in production, embedded from `dist`. In
 * development Vite serves it instead and forwards the admin API to a mock-s3 running on
 * its usual console port, so the app talks to one origin either way and needs no CORS.
 */
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": resolve(import.meta.dirname, "./src"),
    },
  },
  server: {
    port: 5173,
    proxy: {
      "/_admin": {
        target: "http://localhost:9001",
        changeOrigin: true,
      },
    },
  },
  build: {
    // Read by `rust-embed` in the console crate, and by `cargo xtask dist`.
    outDir: "dist",
    emptyOutDir: true,
  },
})
