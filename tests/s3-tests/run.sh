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
# status is not the signal — the comparison below is.
"${compose[@]}" run --rm s3-tests -q --tb=no -rfE "${pytest_args[@]}" \
  | tee "$output" || true

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

if $update; then
  {
    sed -n '1,/^# One test id per line/p' "$known"
    echo
    cat "$actual"
  } > "$known.new"
  mv "$known.new" "$known"
  echo "==> known-failures.txt rewritten from this run"
  exit 0
fi

new_failures="$(comm -23 "$actual" "$expected" || true)"
now_passing="$(comm -13 "$actual" "$expected" || true)"

if [ -n "$now_passing" ]; then
  printf '\n==> these were expected to fail and now pass; remove them from known-failures.txt:\n%s\n' \
    "$now_passing"
fi

if [ -n "$new_failures" ]; then
  printf '\n==> REGRESSION: these failed and are not in known-failures.txt:\n%s\n' \
    "$new_failures" >&2
  exit 1
fi

printf '\n==> no regressions\n'
