import type {
  BucketDetail,
  BucketSummary,
  Health,
  ObjectDetail,
  ObjectListing,
  ObjectSummary,
  RequestEntry,
  ResetReport,
} from "@/types/admin"

/**
 * The admin API client.
 *
 * Same origin in both modes: the binary serves the console from the console port, and in
 * development Vite proxies `/_admin` there. So no base URL and no CORS.
 */
const BASE = "/_admin"

/** An admin API failure, carrying the S3 error code so callers can branch on it. */
export class AdminApiError extends Error {
  readonly code: string
  readonly status: number

  constructor(status: number, code: string, message: string) {
    super(message)
    this.name = "AdminApiError"
    this.code = code
    this.status = status
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`${BASE}${path}`, init)

  if (!response.ok) {
    throw await toError(response)
  }

  if (response.status === 204) {
    return undefined as T
  }

  return (await response.json()) as T
}

/**
 * Turn a failed response into an error worth showing someone.
 *
 * The API answers with `{ error, message }`, but a crashed or proxied request can return
 * anything, so a body that is not that shape falls back to the status rather than
 * throwing a parse error over the real problem.
 */
async function toError(response: Response): Promise<AdminApiError> {
  try {
    const body = (await response.json()) as { error?: string; message?: string }
    if (body.error) {
      return new AdminApiError(response.status, body.error, body.message ?? body.error)
    }
  } catch {
    // Not JSON. The status is all we have.
  }

  return new AdminApiError(response.status, "Unknown", `The admin API answered ${response.status}.`)
}

const json = { "content-type": "application/json" }

export const admin = {
  health: () => request<Health>("/health"),

  buckets: () => request<BucketSummary[]>("/buckets"),

  bucket: (bucket: string) => request<BucketDetail>(`/buckets/${encodeURIComponent(bucket)}`),

  createBucket: (name: string) =>
    request<BucketSummary>("/buckets", {
      method: "POST",
      headers: json,
      body: JSON.stringify({ name }),
    }),

  deleteBucket: (bucket: string) =>
    request<void>(`/buckets/${encodeURIComponent(bucket)}`, { method: "DELETE" }),

  purgeBucket: (bucket: string) =>
    request<ResetReport>(`/reset/${encodeURIComponent(bucket)}`, { method: "POST" }),

  objects: (bucket: string, prefix: string, flat = false) => {
    const params = new URLSearchParams({ prefix })
    if (flat) {
      // An empty delimiter turns folder browsing off and lists every key.
      params.set("delimiter", "")
    }
    return request<ObjectListing>(
      `/buckets/${encodeURIComponent(bucket)}/objects?${params.toString()}`,
    )
  },

  object: (bucket: string, key: string) =>
    request<ObjectDetail>(
      `/buckets/${encodeURIComponent(bucket)}/object?key=${encodeURIComponent(key)}`,
    ),

  uploadObject: (bucket: string, key: string, file: File) =>
    request<ObjectSummary>(
      `/buckets/${encodeURIComponent(bucket)}/object?key=${encodeURIComponent(key)}`,
      {
        method: "PUT",
        // The browser's type for the file, or let the server pick its default.
        headers: file.type ? { "content-type": file.type } : undefined,
        body: file,
      },
    ),

  deleteObject: (bucket: string, key: string) =>
    request<void>(`/buckets/${encodeURIComponent(bucket)}/object?key=${encodeURIComponent(key)}`, {
      method: "DELETE",
    }),

  requests: (limit = 100) => request<RequestEntry[]>(`/requests?limit=${limit}`),
}
