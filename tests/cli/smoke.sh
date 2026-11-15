#!/usr/bin/env bash
#
# The AWS CLI, unmodified, against a running mock-s3. This is the drop-in claim.
#
# The CLI runs in the official container, so nothing has to be installed, and the files
# it reads and writes live in a Docker volume rather than a bind mount — host path
# translation is the one thing guaranteed to behave differently on Windows.
#
# Usage: start mock-s3 on port 9000, then run this.
#   ENDPOINT=http://host.docker.internal:9000 tests/cli/smoke.sh

set -euo pipefail

# Git Bash rewrites arguments that look like Unix paths into Windows ones, which mangles
# container-side paths such as /work. Harmless everywhere else.
export MSYS_NO_PATHCONV=1

ENDPOINT="${ENDPOINT:-http://host.docker.internal:9000}"
CLI_IMAGE="${AWS_CLI_IMAGE:-amazon/aws-cli:latest}"
HELPER_IMAGE="${HELPER_IMAGE:-busybox:latest}"
BUCKET="${BUCKET:-cli-smoke-$$}"
WORK_VOLUME="mock-s3-cli-smoke-$$"

cleanup() {
  docker volume rm -f "$WORK_VOLUME" >/dev/null 2>&1 || true
}
trap cleanup EXIT

# host.docker.internal is built in on Docker Desktop; the add-host makes it resolve on
# Linux too, so the same script works in CI.
aws_cli() {
  docker run --rm \
    --add-host=host.docker.internal:host-gateway \
    -e AWS_ACCESS_KEY_ID=test \
    -e AWS_SECRET_ACCESS_KEY=test \
    -e AWS_DEFAULT_REGION=us-east-1 \
    -v "$WORK_VOLUME:/work" \
    -w /work \
    "$CLI_IMAGE" "$@" --endpoint-url "$ENDPOINT"
}

helper() {
  docker run --rm -v "$WORK_VOLUME:/work" -w /work "$HELPER_IMAGE" sh -c "$1"
}

step() { printf '\n=== %s ===\n' "$1"; }

docker volume create "$WORK_VOLUME" >/dev/null

step "fixture"
helper 'head -c 65536 /dev/urandom > /work/my-file && md5sum /work/my-file'

step "1. create a bucket"
aws_cli s3api create-bucket --bucket "$BUCKET"

step "2. upload a file"
aws_cli s3api put-object --bucket "$BUCKET" --key my-file --body ./my-file

step "3. download the file"
aws_cli s3api get-object --bucket "$BUCKET" --key my-file ./output-file

step "4. the bytes survived the round trip"
helper 'md5sum /work/my-file /work/output-file'
if ! helper 'cmp /work/my-file /work/output-file'; then
  echo "FAIL: the downloaded file differs from the uploaded one" >&2
  exit 1
fi
echo "identical"

step "5. list the bucket"
aws_cli s3 ls "s3://$BUCKET/"

step "6. s3 cp, the high-level command"
aws_cli s3 cp ./my-file "s3://$BUCKET/copied-file"
aws_cli s3 cp "s3://$BUCKET/copied-file" ./copied-back
if ! helper 'cmp /work/my-file /work/copied-back'; then
  echo "FAIL: s3 cp did not round-trip" >&2
  exit 1
fi
echo "identical"

step "7. clean up"
aws_cli s3api delete-object --bucket "$BUCKET" --key my-file
aws_cli s3api delete-object --bucket "$BUCKET" --key copied-file
aws_cli s3api delete-bucket --bucket "$BUCKET"

printf '\nall CLI checks passed\n'
