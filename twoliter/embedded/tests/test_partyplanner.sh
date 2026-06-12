#!/usr/bin/env bash
#
# Test the `first-party-stack` partition-layout math in `partyplanner` by
# sourcing the script into a sub-shell and asserting on the populated
# `pp_size`/`pp_offset` tables. This complements the Rust-side validator
# tests in `tools/buildsys/src/manifest.rs`, which cover the high-level
# feature validation but not the actual disk geometry that lives in bash.
#
# Run from the repo root via `bash twoliter/embedded/tests/test_partyplanner.sh`.

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PARTYPLANNER="${SCRIPT_DIR}/../partyplanner"

if [[ ! -f "${PARTYPLANNER}" ]]; then
  echo "test_partyplanner: partyplanner not found at ${PARTYPLANNER}" >&2
  exit 1
fi

# We source the helper as a library to make `set_partition_sizes` callable.
# shellcheck source=../partyplanner
. "${PARTYPLANNER}"

pass_count=0
fail_count=0

# Mark a test as passed; print a short progress line.
pass() {
  pass_count=$((pass_count + 1))
  echo "  ok: $1"
}

# Mark a test as failed; print details and continue (so we get a full report).
fail() {
  fail_count=$((fail_count + 1))
  echo "  FAIL: $1" >&2
}

# Assert two strings are equal.
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

# Assert that an associative-array key is *not* set.
assert_unset() {
  local -n arr="$1"
  local key name
  key="$2"
  name="$3"
  if [[ -v "arr[${key}]" ]]; then
    fail "${name}: key '${key}' should not be set, but is '${arr[${key}]}'"
  else
    pass "${name}"
  fi
}

# Assert that a sub-shell command exits non-zero.
assert_fails() {
  local name="$1"
  shift
  if ( "$@" ) >/dev/null 2>&1; then
    fail "${name}: expected non-zero exit but command succeeded"
  else
    pass "${name}"
  fi
}

###############################################################################
# Test 1: 1 GiB stripped-down image (first_party_stack=no) produces the
# expected partition table.
#
# With BIOS_MIB=4, EFI_MIB=5, BOOT_SCALE_FACTOR=20, HASH_SCALE_FACTOR=5:
#   total      = 1024 MiB
#   GPT header = 1
#   BIOS       = 4
#   EFI-A      = 10  (5 * 2)
#   BOOT-A     = 40  (20 * 2)
#   ROOT-A     = total - 1 - 4 - 10 - 40 - 10 - 1 = 958
#   HASH-A     = 10  (5 * 2)
#   GPT footer = 1
###############################################################################
echo "Test 1: first_party_stack=no 1 GiB layout"
declare -A partsize partoff
set_partition_sizes 1 1 unified no partsize partoff no

assert_eq "${partoff[BIOS]}"   "1"   "BIOS offset"
assert_eq "${partsize[BIOS]}"  "4"   "BIOS size"
assert_eq "${partoff[EFI-A]}"  "5"   "EFI-A offset"
assert_eq "${partsize[EFI-A]}" "10"  "EFI-A size"
assert_eq "${partoff[BOOT-A]}" "15"  "BOOT-A offset"
assert_eq "${partsize[BOOT-A]}" "40" "BOOT-A size"
assert_eq "${partoff[ROOT-A]}" "55"  "ROOT-A offset"
assert_eq "${partsize[ROOT-A]}" "958" "ROOT-A size (residual)"
assert_eq "${partoff[HASH-A]}" "1013" "HASH-A offset"
assert_eq "${partsize[HASH-A]}" "10" "HASH-A size"

# The stripped-down layout omits these partitions entirely.
for omitted in RESERVED-A PRIVATE DATA-A DATA-B EFI-B BOOT-B ROOT-B HASH-B RESERVED-B; do
  assert_unset partsize "${omitted}" "partsize[${omitted}] is unset"
  assert_unset partoff  "${omitted}" "partoff[${omitted}] is unset"
done

# Sanity check: offsets sum to total - 1 (GPT footer).
total_used=$((partoff[HASH-A] + partsize[HASH-A]))
assert_eq "${total_used}" "1023" "partitions fill image up to GPT footer"

###############################################################################
# Test 2: 2 GiB stripped-down image scales correctly.
#
# total      = 2048
# BIOS       = 4
# EFI-A      = 10  (fixed across image sizes)
# BOOT-A     = 80  (20 * 2 * 2 GiB)
# HASH-A     = 20  (5 * 2 * 2 GiB)
# ROOT-A     = 2048 - 1 - 4 - 10 - 80 - 20 - 1 = 1932
###############################################################################
echo "Test 2: first_party_stack=no 2 GiB layout"
declare -A partsize2 partoff2
set_partition_sizes 2 1 unified no partsize2 partoff2 no

assert_eq "${partsize2[BIOS]}"   "4"    "2GiB BIOS size"
assert_eq "${partsize2[EFI-A]}"  "10"   "2GiB EFI-A size"
assert_eq "${partsize2[BOOT-A]}" "80"   "2GiB BOOT-A size"
assert_eq "${partsize2[HASH-A]}" "20"   "2GiB HASH-A size"
assert_eq "${partsize2[ROOT-A]}" "1932" "2GiB ROOT-A size (residual)"

###############################################################################
# Test 3: defense-in-depth — first_party_stack=no + in_place_updates=yes is
# rejected.
#
# Run in a sub-shell so the `exit 1` inside `set_partition_sizes` does not
# kill the test runner.
###############################################################################
echo "Test 3: first_party_stack=no rejects in_place_updates=yes"
assert_fails "set_partition_sizes 1 1 unified yes <size> <off> no" \
  bash -c '
    set -eu -o pipefail
    . "'"${PARTYPLANNER}"'"
    declare -A s o
    set_partition_sizes 1 1 unified yes s o no
  '

###############################################################################
# Test 4: defense-in-depth — first_party_stack=no + partition_plan=split is
# rejected.
###############################################################################
echo "Test 4: first_party_stack=no rejects partition_plan=split"
assert_fails "set_partition_sizes 1 1 split no <size> <off> no" \
  bash -c '
    set -eu -o pipefail
    . "'"${PARTYPLANNER}"'"
    declare -A s o
    set_partition_sizes 1 1 split no s o no
  '

###############################################################################
# Test 5: too-small image is rejected with a clear error.
#
# We force a too-small image by overriding BOOT_SCALE_FACTOR so that ROOT-A
# would be non-positive.
###############################################################################
echo "Test 5: too-small first_party_stack=no image is rejected"
assert_fails "first_party_stack=no 1 GiB with absurd boot scale factor" \
  bash -c '
    set -eu -o pipefail
    . "'"${PARTYPLANNER}"'"
    BOOT_SCALE_FACTOR=10000
    declare -A s o
    set_partition_sizes 1 1 unified no s o no
  '

###############################################################################
# Test 6: backward-compat / default — omitting the 7th arg defaults to
# first_party_stack=yes and produces the standard layout (with PRIVATE/DATA
# partitions). This is the polarity-flipped default: direct callers that do
# not pass the flag get the full Bottlerocket layout, matching the new Rust
# default of `first-party-stack = true`.
###############################################################################
echo "Test 6: first_party_stack defaults to 'yes' for the standard layout"
declare -A partsize3 partoff3
set_partition_sizes 2 1 unified no partsize3 partoff3
# We expect PRIVATE to exist in the standard layout.
if [[ -v "partsize3[PRIVATE]" ]]; then
  pass "PRIVATE partition present when first_party_stack is omitted"
else
  fail "PRIVATE partition should exist when first_party_stack is omitted"
fi
if [[ -v "partsize3[DATA-A]" ]]; then
  pass "DATA-A partition present when first_party_stack is omitted"
else
  fail "DATA-A partition should exist when first_party_stack is omitted"
fi

###############################################################################
echo
echo "Results: ${pass_count} passed, ${fail_count} failed"
if [[ "${fail_count}" -gt 0 ]]; then
  exit 1
fi
