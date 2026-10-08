#!/usr/bin/env bash
#
# Test the deterministic-identifier helpers in `imghelper`: `derive_uuid`,
# `derive_fat_volid`, `ext4_hash_seed`. These produce stable UUIDs/volume IDs
# from `BUILD_ID_TIMESTAMP`, `VARIANT`, `ARCH`, and an `id_target` tag, and are the
# foundation of reproducible filesystem creation.
#
# Run from the repo root via `bash twoliter/embedded/tests/test_deterministic_ids.sh`.

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMGHELPER="${SCRIPT_DIR}/../imghelper"

if [[ ! -f "${IMGHELPER}" ]]; then
  echo "test_deterministic_ids: imghelper not found at ${IMGHELPER}" >&2
  exit 1
fi

# Set the minimum environment that `imghelper` reads at source time so sourcing
# the file succeeds. The deterministic-ID helpers themselves only consume
# BUILD_ID_TIMESTAMP, VARIANT, and ARCH, but `imghelper` references other
# variables unconditionally at the top of the file.
export IMAGE_NAME=test VARIANT=test ARCH=x86_64 VERSION_ID=1.0 BUILD_ID=0

# shellcheck source=../imghelper
. "${IMGHELPER}"

pass_count=0
fail_count=0

pass() {
  pass_count=$((pass_count + 1))
  echo "  ok: $1"
}

fail() {
  fail_count=$((fail_count + 1))
  echo "  FAIL: $1" >&2
}

assert_eq() {
  local actual expected name
  actual="$1"
  expected="$2"
  name="$3"
  if [[ "${actual}" == "${expected}" ]]; then
    pass "${name} (got '${actual}')"
  else
    fail "${name}: expected '${expected}' got '${actual}'"
  fi
}

assert_ne() {
  local actual other name
  actual="$1"
  other="$2"
  name="$3"
  if [[ "${actual}" != "${other}" ]]; then
    pass "${name} ('${actual}' != '${other}')"
  else
    fail "${name}: expected values to differ, both were '${actual}'"
  fi
}

assert_match() {
  local actual pattern name
  actual="$1"
  pattern="$2"
  name="$3"
  if [[ "${actual}" =~ ${pattern} ]]; then
    pass "${name} ('${actual}' matches ${pattern})"
  else
    fail "${name}: '${actual}' does not match ${pattern}"
  fi
}

UUID_RE='^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$'
FAT_VOLID_RE='^[0-9a-f]{8}$'

###############################################################################
echo "Test 1: derive_uuid output shape and determinism"
###############################################################################

BUILD_ID_TIMESTAMP=1700000000 VARIANT=foo ARCH=x86_64 \
  u1="$(derive_uuid root-ext4)"
BUILD_ID_TIMESTAMP=1700000000 VARIANT=foo ARCH=x86_64 \
  u2="$(derive_uuid root-ext4)"

assert_match "${u1}" "${UUID_RE}" "derive_uuid shape"
assert_eq "${u1}" "${u2}" "derive_uuid deterministic for same inputs"

###############################################################################
echo "Test 2: derive_uuid is sensitive to each input"
###############################################################################

BUILD_ID_TIMESTAMP=1700000000 VARIANT=bar ARCH=x86_64 \
  u_other_variant="$(derive_uuid root-ext4)"
assert_ne "${u1}" "${u_other_variant}" "VARIANT changes derive_uuid output"

BUILD_ID_TIMESTAMP=1700000000 VARIANT=foo ARCH=aarch64 \
  u_other_arch="$(derive_uuid root-ext4)"
assert_ne "${u1}" "${u_other_arch}" "ARCH changes derive_uuid output"

BUILD_ID_TIMESTAMP=1700000001 VARIANT=foo ARCH=x86_64 \
  u_other_ts="$(derive_uuid root-ext4)"
assert_ne "${u1}" "${u_other_ts}" "BUILD_ID_TIMESTAMP changes derive_uuid output"

BUILD_ID_TIMESTAMP=1700000000 VARIANT=foo ARCH=x86_64 \
  u_other_id_target="$(derive_uuid boot-ext4)"
assert_ne "${u1}" "${u_other_id_target}" "id_target changes derive_uuid output"

###############################################################################
echo "Test 3: derive_fat_volid output shape and determinism"
###############################################################################

BUILD_ID_TIMESTAMP=1700000000 VARIANT=foo ARCH=x86_64 \
  v1="$(derive_fat_volid efi-vfat)"
BUILD_ID_TIMESTAMP=1700000000 VARIANT=foo ARCH=x86_64 \
  v2="$(derive_fat_volid efi-vfat)"

assert_match "${v1}" "${FAT_VOLID_RE}" "derive_fat_volid shape"
assert_eq "${v1}" "${v2}" "derive_fat_volid deterministic for same inputs"

BUILD_ID_TIMESTAMP=1700000000 VARIANT=foo ARCH=x86_64 \
  v_other_id_target="$(derive_fat_volid uki-efi-vfat)"
assert_ne "${v1}" "${v_other_id_target}" "id_target changes derive_fat_volid output"

###############################################################################
echo "Test 4: ext4_hash_seed is a UUID"
###############################################################################

# shellcheck disable=SC2034 # read by `ext4_hash_seed` in the subshell below
BUILD_ID_TIMESTAMP=1700000000 VARIANT=foo ARCH=x86_64 \
  seed1="$(ext4_hash_seed root-ext4)"
assert_match "${seed1}" "${UUID_RE}" "ext4_hash_seed shape"
assert_eq "${seed1}" "${u1}" "ext4_hash_seed matches derive_uuid for same id_target"

###############################################################################
echo "Test 5: missing required env is a hard error"
###############################################################################

if bash -c "
  set -u -o pipefail
  export IMAGE_NAME=test VARIANT=test ARCH=x86_64 VERSION_ID=1.0 BUILD_ID=0
  unset BUILD_ID_TIMESTAMP
  . '${IMGHELPER}'
  derive_uuid root-ext4
" >/dev/null 2>&1; then
  fail "derive_uuid did not fail with BUILD_ID_TIMESTAMP unset"
else
  pass "derive_uuid fails when BUILD_ID_TIMESTAMP is unset"
fi

if bash -c "
  set -u -o pipefail
  export IMAGE_NAME=test ARCH=x86_64 VERSION_ID=1.0 BUILD_ID=0 BUILD_ID_TIMESTAMP=1
  unset VARIANT
  . '${IMGHELPER}'
  derive_uuid root-ext4
" >/dev/null 2>&1; then
  fail "derive_uuid did not fail with VARIANT unset"
else
  pass "derive_uuid fails when VARIANT is unset"
fi

###############################################################################
echo
echo "Results: ${pass_count} passed, ${fail_count} failed"
if [[ "${fail_count}" -gt 0 ]]; then
  exit 1
fi
