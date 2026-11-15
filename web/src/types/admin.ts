/** The shapes the mock-s3 admin API answers with. Mirrors `mock-s3-console/src/lib.rs`. */

export type Health = {
  status: string
  region: string
  /** "memory", or the data directory. Tells you whether objects survive a restart. */
  backend: string
  buckets: number
}

export type BucketSummary = {
  name: string
  created_at: string
  objects: number
  bytes: number
}

export type VersioningStatus = "Unversioned" | "Enabled" | "Suspended"

export type BucketDetail = BucketSummary & {
  versioning: VersioningStatus
}

export type ObjectSummary = {
  key: string
  size: number
  etag: string
  last_modified: string
  content_type: string
}

export type ObjectListing = {
  /** Keys rolled up under the delimiter, so a key space reads as folders. */
  prefixes: string[]
  objects: ObjectSummary[]
  is_truncated: boolean
}

export type ObjectVersionSummary = {
  version_id: string
  is_latest: boolean
  is_delete_marker: boolean
  last_modified: string
  size: number
  etag: string
}

export type ObjectDetail = ObjectSummary & {
  metadata: Record<string, string>
  versions: ObjectVersionSummary[]
}

export type RequestEntry = {
  at: string
  method: string
  path: string
  query?: string
  status: number
  duration_ms: number
  bucket?: string
  key?: string
  access_key_id?: string
  request_id?: string
}

export type ResetReport = {
  reset: string
  objects_removed: number | null
}
