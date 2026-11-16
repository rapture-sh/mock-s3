#!/usr/bin/env bash
#
# Run ceph/s3-tests against mock-s3 and compare the result with known-failures.txt.
#
# Opt-in: it is slow, it needs the network to build, and most of the suite covers
# features a mock deliberately does not have. It exists to give an outside measure of
# compatibility rather than to gate development.
#
#   tests/s3-tests/run.sh                    # compare against the known failures
#   tests/s3-tests/run.sh --update           # rewrite the list from this run
#   tests/s3-tests/run.sh -- -k test_bucket  # pass arguments through to pytest
#
# Anything after `--` goes to pytest, so a single test or a narrow -k selection is the
# fastest way to check one behaviour.

set -euo pipefail

# Deliberately no MSYS_NO_PATHCONV here, unlike tests/cli/smoke.sh: every path below is a
# host path that Git Bash must keep converting for docker.exe. Disabling the conversion
# turns /c/Users/... into C:\c\Users\... and nothing is found.

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
known="$here/known-failures.txt"

compose=(docker compose -f "$root/docker-compose.s3tests.yml")

# Why each remaining failure is expected, tried in order: the first reason whose pattern
# matches a test id claims it. Keeping the patterns here rather than beside each line is
# what lets `--update` regenerate the file without throwing the reasons away.
#
# A failure that matches nothing lands under "not yet triaged", which is the honest
# answer for one nobody has looked at, never a reason invented to make the list tidy.
# Shrinking that section, by fixing a test or by giving it a reason, is the work.
reasons=(
  "object lock, retention and legal hold are not implemented@@object_lock|retention|legal_hold|governance|compliance"
  "no encryption: mock-s3 adds no SSE-C or SSE-KMS, deliberately, see SECURITY.md@@sse|encrypt|kms|_enc_|customer_key"
  "no signature verification: any credentials are accepted, so a bad or expired one is not detected@@_auth|expires_out|authenticated_expired|presigned"
  "a key that is not valid UTF-8 is refused rather than stored@@unreadable"
  "object ownership controls are not enforced@@ownership|bucket_owner|object_writer"
  "public access block is not enforced@@public_block|public_buckets"
  "no ACL enforcement: any credentials are accepted and no grant is evaluated@@acl|grant|_acp|public_?read|public_?write|authenticated_request|anon|access_bucket|not_owned|nonowner"
  "no bucket policy or IAM evaluation@@policy|principal|expected_bucket_owner"
  "no second account: the suite needs two real principals@@tenant|cross_account|different_account|altaccount"
  "browser form POST uploads are not implemented@@post_object|browser"
  "bucket logging is not implemented@@logging"
  "no lifecycle engine: no expiration or transition@@lifecycle|marker_expiration"
  "tagging is accepted and ignored, never stored@@tagging|tagset|_tags"
  "website hosting is not implemented@@website"
  "replication is not implemented@@replication"
  "UploadPartCopy is not implemented@@upload_part_copy|multipart_copy|copy_part"
  "S3 Select is not implemented@@select"
  "GetObjectAttributes is not implemented@@object_attributes"
  "inventory, analytics and metrics configurations are not implemented@@inventory|analytics|metrics|intelligent"
  "conditional writes are not implemented: If-Match and If-None-Match on put, copy and delete@@if_match|ifmatch|if_none_match|ifnonematch|ifnonmatch|conditional_write"
  "checksum algorithms and trailer variants are not implemented@@checksum|cksum|trailer"
  "partNumber on GET is not implemented@@get_part"
  "a ceph extension rather than real S3 behaviour@@usage|unordered|head_extended"
  "Expect: 100-continue is not implemented@@100_continue"
  "the 1000-key limit on DeleteObjects is not enforced@@delete_key_limit"
  "ListBuckets pagination is not implemented@@list_buckets_paginated"
  "torrent is not implemented@@torrent"
  "one region only: a LocationConstraint is accepted and ignored@@bucket_get_location"
  "no ACL enforcement: the test needs a public-read bucket to refuse a write@@cors_origin_response"
  "no second account: the suite needs two real principals@@multipart_upload_owner"
  "a duplicate part number in CompleteMultipartUpload is refused, not resolved to the last write@@resend_first_finishes_last"
)

# Rewrite a sorted list of test ids as the same list grouped under its reasons.
group_by_reason() {
  local source="$1" destination="$2"
  local remaining matched reason pattern
  remaining="$(mktemp)"
  cp "$source" "$remaining"
  : > "$destination"

  for entry in "${reasons[@]}"; do
    reason="${entry%%@@*}"
    pattern="${entry##*@@}"
    matched="$(mktemp)"
    { grep -E "$pattern" "$remaining" || true; } | sort -u > "$matched"

    if [ -s "$matched" ]; then
      printf '\n# %s (%s)\n' "$reason" "$(wc -l < "$matched" | tr -d ' ')" >> "$destination"
      cat "$matched" >> "$destination"
      comm -23 "$remaining" "$matched" > "$remaining.keep"
      mv "$remaining.keep" "$remaining"
    fi
    rm -f "$matched"
  done

  if [ -s "$remaining" ]; then
    printf '\n# not yet triaged (%s)\n' "$(wc -l < "$remaining" | tr -d ' ')" >> "$destination"
    cat "$remaining" >> "$destination"
  fi
  rm -f "$remaining"
}
update=false
pytest_args=()

while [ $# -gt 0 ]; do
  case "$1" in
    --update) update=true; shift ;;
    --) shift; pytest_args=("$@"); break ;;
    *) pytest_args+=("$1"); shift ;;
  esac
done

if [ ${#pytest_args[@]} -eq 0 ]; then
  pytest_args=(s3tests/functional/test_s3.py)
fi

cleanup() {
  "${compose[@]}" down --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> building (first run clones the suite and installs its dependencies)"
"${compose[@]}" build

echo "==> starting mock-s3"
"${compose[@]}" up -d mock-s3

echo "==> running ${pytest_args[*]}"
output="$(mktemp)"
# pytest exits non-zero whenever anything failed, which is the normal case here, so its
# status is not the signal, the comparison below is.
"${compose[@]}" run --rm s3-tests -q --tb=no -rfE "${pytest_args[@]}" \
  | tee "$output" || true

# A run that died, killed, out of memory, the server never came up, prints no summary
# at all, and every check below reads that as "nothing failed". In `--update` that writes
# an empty known-failures.txt, and without it the script reports no regressions and exits
# 0. Both turn a broken run into a perfect score, so refuse before reading the results.
if ! grep -qE '^[0-9]+ (passed|failed)|no tests ran|^=+ .*(passed|failed|no tests ran)' "$output"; then
  printf '\n==> the suite did not finish: no pytest summary in its output.\n' >&2
  printf '    Nothing was compared and known-failures.txt was left alone.\n' >&2
  exit 1
fi

# `-rfE` puts a `FAILED` or `ERROR` line in the short summary for each one. Without the
# E, setup and teardown errors are invisible here and would not count as regressions.
actual="$(mktemp)"
# grep exits 1 when it matches nothing, which under `set -e` would abort the script on
# the very outcome this is working towards: a run with no failures at all.
{ grep -E '^(FAILED|ERROR) ' "$output" || true; } \
  | sed -E 's/^(FAILED|ERROR) //; s/ - .*$//' \
  | sort -u > "$actual"

expected="$(mktemp)"
# Same again: a known-failures file that is only comments matches nothing.
{ grep -vE '^\s*(#|$)' "$known" || true; } | sort -u > "$expected"

printf '\n==> %s failing, %s expected to fail\n' \
  "$(wc -l < "$actual")" "$(wc -l < "$expected")"

new_failures="$(comm -23 "$actual" "$expected" || true)"
now_passing="$(comm -13 "$actual" "$expected" || true)"

if [ -n "$now_passing" ]; then
  printf '\n==> these were expected to fail and now pass:\n%s\n' "$now_passing"
fi

# Reported before the rewrite, not instead of it. A reseed that absorbs a regression
# without saying so would quietly raise the bar it is supposed to be lowering, and the
# count below is what makes it visible in review even when the diff is long.
if [ -n "$new_failures" ]; then
  printf '\n==> REGRESSION: these failed and are not in known-failures.txt:\n%s\n' \
    "$new_failures" >&2
fi

if $update; then
  grouped="$(mktemp)"
  group_by_reason "$actual" "$grouped"
  {
    sed -n '1,/^# One test id per line/p' "$known"
    cat "$grouped"
  } > "$known.new"
  mv "$known.new" "$known"
  rm -f "$grouped"

  printf '\n==> known-failures.txt rewritten: %s removed, %s added\n' \
    "$(printf '%s' "$now_passing" | grep -c . || true)" \
    "$(printf '%s' "$new_failures" | grep -c . || true)"
  printf '\n==> by reason:\n'
  { grep -E '^# .*\([0-9]+\)$' "$known" || true; } | sed 's/^# /    /'
  exit 0
fi

if [ -n "$new_failures" ]; then
  exit 1
fi

printf '\n==> no regressions\n'
