/** Formatting for the things this console shows: sizes, durations, timestamps. */

const UNITS = ["B", "KB", "MB", "GB", "TB"] as const

/**
 * A byte count at a glance.
 *
 * Decimal units, matching what the AWS console and `aws s3 ls` report, so a size here
 * matches the size a developer sees elsewhere rather than disagreeing by 2.4%.
 */
export function formatBytes(bytes: number): string {
  if (bytes === 0) {
    return "0 B"
  }

  const exponent = Math.min(Math.floor(Math.log10(bytes) / 3), UNITS.length - 1)
  const value = bytes / 1000 ** exponent
  const digits = exponent === 0 || value >= 100 ? 0 : 1

  return `${value.toFixed(digits)} ${UNITS[exponent]}`
}

/** A request duration. Sub-millisecond is the common case on a local instance. */
export function formatDuration(milliseconds: number): string {
  if (milliseconds === 0) {
    return "<1 ms"
  }

  if (milliseconds < 1000) {
    return `${milliseconds} ms`
  }

  return `${(milliseconds / 1000).toFixed(2)} s`
}

/**
 * How long ago, in the shortest form that is still unambiguous.
 *
 * A console open beside a test run is mostly looking at the last few seconds, which is
 * why this goes finer than it goes coarse.
 */
export function formatRelative(iso: string, now = Date.now()): string {
  const then = new Date(iso).getTime()

  if (Number.isNaN(then)) {
    return "unknown"
  }

  const seconds = Math.max(0, Math.round((now - then) / 1000))

  if (seconds < 5) {
    return "just now"
  }
  if (seconds < 60) {
    return `${seconds}s ago`
  }
  if (seconds < 3600) {
    return `${Math.floor(seconds / 60)}m ago`
  }
  if (seconds < 86400) {
    return `${Math.floor(seconds / 3600)}h ago`
  }

  return `${Math.floor(seconds / 86400)}d ago`
}

/** A full timestamp, for a tooltip behind the relative one. */
export function formatExact(iso: string): string {
  const date = new Date(iso)

  if (Number.isNaN(date.getTime())) {
    return iso
  }

  return date.toLocaleString(undefined, {
    dateStyle: "medium",
    timeStyle: "medium",
  })
}

/** The last segment of a key, which is what a folder view shows as the file name. */
export function keyLeaf(key: string, prefix: string): string {
  return key.slice(prefix.length)
}

/** A prefix as a folder name: `photos/2024/` under `photos/` reads as `2024/`. */
export function prefixLeaf(prefix: string, parent: string): string {
  return prefix.slice(parent.length)
}

/**
 * Split a prefix into the crumbs a breadcrumb needs, each carrying the full prefix to
 * navigate to.
 */
export function prefixCrumbs(prefix: string): { label: string; prefix: string }[] {
  if (prefix === "") {
    return []
  }

  const parts = prefix.split("/").filter((part) => part !== "")

  return parts.map((part, index) => ({
    label: part,
    prefix: `${parts.slice(0, index + 1).join("/")}/`,
  }))
}
