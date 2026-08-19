#!/usr/bin/env bash
#
# Test `get_fat_usage` in `imghelper`:
#
#   1. The emitted JSON is keyed by the given partition name and carries
#      exactly the same fields, in the same order, as `get_erofs_usage` and
#      `get_ext4_usage`, since all three feed the same
#      `jq -s 'add | {disk_usage: .}'` merge in `img2img` and `rpm2img`.
#   2. `bytes_total` is computed from the partition size argument rather than
#      read back from the image file, matching the `get_erofs_usage`
#      signature.
#   3. `bytes_used` reflects real FAT allocation accounting rather than a
#      single file's size, so it rises when a file is added to the image.
#
# Run from the repo root:
#   bash twoliter/embedded/tests/test_fat_usage.sh

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMGHELPER="${SCRIPT_DIR}/../imghelper"

if [[ ! -f "${IMGHELPER}" ]]; then
  echo "test_fat_usage: imghelper not found at ${IMGHELPER}" >&2
  exit 1
fi

# imghelper's top-level `${VAR:?}` expansions require these to be set before
# it can be sourced, even though this test only exercises `get_fat_usage`.
IMAGE_NAME=x VARIANT=x ARCH=x86_64 VERSION_ID=x BUILD_ID=x
export IMAGE_NAME VARIANT ARCH VERSION_ID BUILD_ID
# shellcheck source=../imghelper
. "${IMGHELPER}"

pass_count=0
fail_count=0

pass() { pass_count=$((pass_count + 1)); echo "  ok: $1"; }
fail() { fail_count=$((fail_count + 1)); echo "  FAIL: $1" >&2; }
assert_eq() {
  local got want label
  got="$1"
  want="$2"
  label="$3"
  if [[ "${got}" == "${want}" ]]; then
    pass "${label}"
  else
    fail "${label}: got '${got}', want '${want}'"
  fi
}

tmp=$(mktemp -d)
trap 'rm -rf "${tmp}"' EXIT

# Stand in for the FAT32 BOOT-A image that `mkfs_boot_uki` builds with
# `mkfs.vfat`. `mformat` is used here because it needs no privileges and comes
# from the same `mtools` package as the `mdir` call under test.
part_mib=20
image="${tmp}/boot.img"
dd if=/dev/zero of="${image}" bs=1M count="${part_mib}" status=none
mformat -i "${image}" -F -v BOOTA ::
mmd -i "${image}" ::/EFI
mmd -i "${image}" ::/EFI/Linux

# ---------------------------------------------------------------------------
# Test 1: shape and keys match get_erofs_usage's output for the same
# partition name and size, with only bytes_used and its dependents differing.
# ---------------------------------------------------------------------------
usage="$(get_fat_usage "${image}" "BOOT-A" "${part_mib}")"

partition_key="$(echo "${usage}" | jq -r 'keys[0]')"
assert_eq "${partition_key}" "BOOT-A" "output is keyed by the given partition name"

fields="$(echo "${usage}" | jq -r '.["BOOT-A"] | keys | join(",")')"
assert_eq "${fields}" "bytes_remaining,bytes_total,bytes_used,percentage_bytes" "output has exactly the four expected fields"

bytes_total="$(echo "${usage}" | jq -r '.["BOOT-A"].bytes_total')"
assert_eq "${bytes_total}" "$((part_mib * 1024 * 1024))" "bytes_total is partition_size_mib times 1024 times 1024"

# ---------------------------------------------------------------------------
# Test 2: bytes_used, bytes_remaining, and percentage_bytes are internally
# consistent with bytes_total, and bytes_used is nonzero even though no
# regular file has been copied in yet, since directory entries and reserved
# regions already consume space.
# ---------------------------------------------------------------------------
bytes_used="$(echo "${usage}" | jq -r '.["BOOT-A"].bytes_used')"
bytes_remaining="$(echo "${usage}" | jq -r '.["BOOT-A"].bytes_remaining')"
percentage_bytes="$(echo "${usage}" | jq -r '.["BOOT-A"].percentage_bytes')"

assert_eq "$((bytes_used + bytes_remaining))" "${bytes_total}" "bytes_used plus bytes_remaining equals bytes_total"
assert_eq "${percentage_bytes}" "$((bytes_used * 100 / bytes_total))" "percentage_bytes is the truncating integer percent of bytes_used"

if [[ "${bytes_used}" -gt 0 ]]; then
  pass "bytes_used is nonzero before any file is copied in, reflecting FAT overhead"
else
  fail "bytes_used should be nonzero due to reserved sectors, FAT tables, and directory entries"
fi

# ---------------------------------------------------------------------------
# Test 3: bytes_used increases once a file is copied into the image, and by
# at least the file's own size.
# ---------------------------------------------------------------------------
dd if=/dev/urandom of="${tmp}/bottlerocket.efi" bs=1K count=500 status=none
mcopy -i "${image}" "${tmp}/bottlerocket.efi" ::/EFI/Linux/

usage_after="$(get_fat_usage "${image}" "BOOT-A" "${part_mib}")"
bytes_used_after="$(echo "${usage_after}" | jq -r '.["BOOT-A"].bytes_used')"

if [[ "${bytes_used_after}" -ge "$((bytes_used + 500 * 1024))" ]]; then
  pass "bytes_used grows by at least the copied file's size"
else
  fail "bytes_used_after (${bytes_used_after}) should be at least bytes_used (${bytes_used}) plus the file size"
fi

# ---------------------------------------------------------------------------
echo
echo "test_fat_usage: ${pass_count} passed, ${fail_count} failed"
[[ "${fail_count}" -eq 0 ]]
