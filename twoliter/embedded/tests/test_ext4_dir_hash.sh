#!/usr/bin/env bash
#
# Tests `ext4_set_signed_dir_hash` in `imghelper`; skipped without e2fsprogs.
# Run from the repo root via `bash twoliter/embedded/tests/test_ext4_dir_hash.sh`.

set -eu -o pipefail

IMGHELPER="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/../imghelper"

die() {
  echo "test_ext4_dir_hash: FAIL: $1" >&2
  exit 1
}

[[ -f "${IMGHELPER}" ]] || die "imghelper not found at ${IMGHELPER}"
for tool in mkfs.ext4 debugfs dumpe2fs e2fsck; do
  if ! command -v "${tool}" >/dev/null; then
    echo "test_ext4_dir_hash: ${tool} not found; skipping"
    exit 0
  fi
done

export IMAGE_NAME=test VARIANT=test ARCH=x86_64 VERSION_ID=1.0 BUILD_ID=0
# shellcheck source=../imghelper
. "${IMGHELPER}"

# Keep the timestamps debugfs and mkfs.ext4 write stable, as the image build does.
export SOURCE_DATE_EPOCH=1700000000 E2FSPROGS_FAKE_TIME=1700000000

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "${WORK_DIR}"' EXIT

# Build an ext4 image of <dir> at <image> with fixed identifiers.
make_image() {
  rm -f "${2:?}"
  mkfs.ext4 -q -b 4096 -U 3f0c9a64-2b1e-4c55-9d7a-6f3e1c2b8a90 \
    -E hash_seed=3f0c9a64-2b1e-4c55-9d7a-6f3e1c2b8a90 -d "${1:?}" "${2}" 8M
}

# Enough entries that the directory gets an htree index.
ASCII_DIR="${WORK_DIR}/ascii"
mkdir -p "${ASCII_DIR}/many"
for i in $(seq 1 200); do
  : >"${ASCII_DIR}/many/settings-entry-${i}"
done
find "${ASCII_DIR}" -exec touch -h -d "@${SOURCE_DATE_EPOCH}" {} +
# Copied for each image: before e2fsprogs 1.47.1, `mkfs.ext4 -d` copies the
# tree's atimes, which reading it changes.
make_image "${ASCII_DIR}" "${WORK_DIR}/pristine.img"

echo "Test 1: images from signed- and unsigned-char hosts come out identical and signed"
cp "${WORK_DIR}/pristine.img" "${WORK_DIR}/a.img"
cp "${WORK_DIR}/pristine.img" "${WORK_DIR}/b.img"
echo "ssv flags 2" | debugfs -w -f - "${WORK_DIR}/b.img" >/dev/null 2>&1
for image in a b; do
  ext4_set_signed_dir_hash "${WORK_DIR}/${image}.img" "${ASCII_DIR}" >/dev/null 2>&1
done
flags="$(dumpe2fs -h "${WORK_DIR}/a.img" 2>/dev/null | sed -n 's/^Filesystem flags: *//p' | xargs)"
[[ "${flags}" == "signed_directory_hash" ]] || die "expected signed_directory_hash, got '${flags}'"
cmp "${WORK_DIR}/a.img" "${WORK_DIR}/b.img" || die "images differ"
e2fsck -fn "${WORK_DIR}/b.img" >/dev/null 2>&1 || die "e2fsck reports errors"

echo "Test 2: names with non-ASCII bytes are refused, leaving the image unchanged"
NON_ASCII_DIR="${WORK_DIR}/non-ascii"
mkdir -p "${NON_ASCII_DIR}"
: >"${NON_ASCII_DIR}/caf"$'\xc3\xa9'
make_image "${NON_ASCII_DIR}" "${WORK_DIR}/c.img"
cp "${WORK_DIR}/c.img" "${WORK_DIR}/c-before.img"
if ext4_set_signed_dir_hash "${WORK_DIR}/c.img" "${NON_ASCII_DIR}" >/dev/null 2>&1; then
  die "ext4_set_signed_dir_hash accepted a non-ASCII name"
fi
cmp -s "${WORK_DIR}/c.img" "${WORK_DIR}/c-before.img" || die "the image was changed"

echo "All tests passed"
