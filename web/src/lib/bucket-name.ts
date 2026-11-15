/**
 * The S3 bucket naming rules, for telling someone what is wrong while they type.
 *
 * This mirrors `validate_bucket_name` in `mock-s3-core/src/model.rs`, which is what
 * actually decides: the server is authoritative and will refuse a bad name whatever this
 * says. The copy exists so the modal can explain the problem before a round trip, not so
 * the check lives in two places — if the two ever disagree, the Rust one is right.
 */

const MIN = 3
const MAX = 63

export function validateBucketName(name: string): string | undefined {
  if (name.length < MIN || name.length > MAX) {
    return `Use between ${MIN} and ${MAX} characters.`
  }

  if (!/^[a-z0-9.-]+$/.test(name)) {
    return "Use lowercase letters, numbers, hyphens and dots only."
  }

  for (const label of name.split(".")) {
    if (label === "") {
      return "Dots cannot be next to each other, or at the start or end."
    }

    if (!/^[a-z0-9]/.test(label) || !/[a-z0-9]$/.test(label)) {
      return "Each part between dots must start and end with a letter or number."
    }
  }

  if (isIpAddress(name)) {
    return "A name cannot look like an IP address."
  }

  if (name.startsWith("xn--")) {
    return "A name cannot start with xn--."
  }

  if (name.endsWith("-s3alias")) {
    return "A name cannot end with -s3alias."
  }

  return undefined
}

function isIpAddress(name: string): boolean {
  const labels = name.split(".")

  return labels.length === 4 && labels.every((label) => /^\d{1,3}$/.test(label))
}
