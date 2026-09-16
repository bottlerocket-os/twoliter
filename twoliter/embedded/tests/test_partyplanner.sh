#!/usr/bin/env bash
#
# Test the `standalone-image` partition-layout math in `partyplanner` by
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
# Test 1: 1 GiB stripped-down image (standalone_image=yes) produces the
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
echo "Test 1: standalone_image=yes 1 GiB layout"
declare -A partsize partoff
set_partition_sizes 1 1 unified no partsize partoff yes

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
echo "Test 2: standalone_image=yes 2 GiB layout"
declare -A partsize2 partoff2
set_partition_sizes 2 1 unified no partsize2 partoff2 yes

assert_eq "${partsize2[BIOS]}"   "4"    "2GiB BIOS size"
assert_eq "${partsize2[EFI-A]}"  "10"   "2GiB EFI-A size"
assert_eq "${partsize2[BOOT-A]}" "80"   "2GiB BOOT-A size"
assert_eq "${partsize2[HASH-A]}" "20"   "2GiB HASH-A size"
assert_eq "${partsize2[ROOT-A]}" "1932" "2GiB ROOT-A size (residual)"

###############################################################################
# Test 3: defense-in-depth — standalone_image=yes + in_place_updates=yes is
# rejected.
#
# Run in a sub-shell so the `exit 1` inside `set_partition_sizes` does not
# kill the test runner.
###############################################################################
echo "Test 3: standalone_image=yes rejects in_place_updates=yes"
assert_fails "set_partition_sizes 1 1 unified yes <size> <off> yes" \
  bash -c '
    set -eu -o pipefail
    . "'"${PARTYPLANNER}"'"
    declare -A s o
    set_partition_sizes 1 1 unified yes s o yes
  '

###############################################################################
# Test 4: defense-in-depth — standalone_image=yes + partition_plan=split is
# rejected.
###############################################################################
echo "Test 4: standalone_image=yes rejects partition_plan=split"
assert_fails "set_partition_sizes 1 1 split no <size> <off> yes" \
  bash -c '
    set -eu -o pipefail
    . "'"${PARTYPLANNER}"'"
    declare -A s o
    set_partition_sizes 1 1 split no s o yes
  '

###############################################################################
# Test 5: too-small image is rejected with a clear error.
#
# We force a too-small image by overriding BOOT_SCALE_FACTOR so that ROOT-A
# would be non-positive.
###############################################################################
echo "Test 5: too-small standalone_image=yes image is rejected"
assert_fails "standalone_image=yes 1 GiB with absurd boot scale factor" \
  bash -c '
    set -eu -o pipefail
    . "'"${PARTYPLANNER}"'"
    BOOT_SCALE_FACTOR=10000
    declare -A s o
    set_partition_sizes 1 1 unified no s o yes
  '

###############################################################################
# Test 6: backward-compat / default — omitting the 7th arg defaults to
# standalone_image=no and produces the standard layout (with PRIVATE/DATA
# partitions). Direct callers that do not pass the flag get the full
# Bottlerocket layout, matching the new Rust default of
# `standalone-image = false`.
###############################################################################
echo "Test 6: standalone_image defaults to 'no' for the standard layout"
declare -A partsize3 partoff3
set_partition_sizes 2 1 unified no partsize3 partoff3
# We expect PRIVATE to exist in the standard layout.
if [[ -v "partsize3[PRIVATE]" ]]; then
  pass "PRIVATE partition present when standalone_image is omitted"
else
  fail "PRIVATE partition should exist when standalone_image is omitted"
fi
if [[ -v "partsize3[DATA-A]" ]]; then
  pass "DATA-A partition present when standalone_image is omitted"
else
  fail "DATA-A partition should exist when standalone_image is omitted"
fi

###############################################################################
# Test 6b: merged UKI layout (uki_image=yes) boots the EFI directly from ESP.
# There is NO separate BOOT-A partition; and the ESP is placed immediately before
# ROOT-A. The overall image size is unchanged.
#
# 2 GiB split image, single-bank (UKI is incompatible with in-place-updates):
#   EFI-A      = (5*2) + (40*2)         = 90  (was 10 = EFI_MIB * 2)
#   BOOT-A     = (dropped entirely)
#   RESERVED-A = (2*15 - 5)*2           = 50
# Every partition from ROOT-A onward keeps the same size as the non-UKI
# single-bank 2 GiB split layout; only EFI-A grows and BOOT-A vanishes.
###############################################################################
echo "Test 6b: merged UKI layout drops BOOT-A"
declare -A uki_size uki_off non_uki_size non_uki_off
set_partition_sizes 2 1 split no uki_size uki_off no yes
set_partition_sizes 2 1 split no non_uki_size non_uki_off no no

assert_eq "${uki_size[EFI-A]}"      "90" "UKI EFI-A size == 2*EFI_MIB size + 2*BOOT_A size"
assert_eq "${non_uki_size[EFI-A]}"  "10" "non-UKI EFI-A size unchanged"

# The merged UKI layout has NO BOOT-A partition at all.
assert_eq "${uki_size[BOOT-A]:-0}" "0" "UKI layout allocates no space for BOOT-A"

# RESERVED-A stays the same.
assert_eq "${uki_size[RESERVED-A]}" "50" "UKI RESERVED-A stays the same"
assert_eq "${non_uki_size[RESERVED-A]}" "50" "non-UKI RESERVED-A unchanged"

# The ESP must be large enough to hold the ~15 MiB UKI with headroom.
if (( uki_size[EFI-A] >= 15 )); then
  pass "UKI EFI-A (${uki_size[EFI-A]} MiB) >= 15 MiB UKI"
else
  fail "UKI EFI-A (${uki_size[EFI-A]} MiB) too small for a ~15 MiB UKI"
fi

# EFI-A offset is unchanged (it still follows BIOS).
assert_eq "${uki_off[EFI-A]}" "${non_uki_off[EFI-A]}" "EFI-A offset unchanged"

# ROOT-A must sit immediately after the ESP (ESP+1): no BOOT-A between them.
assert_eq "${uki_off[ROOT-A]}" "$((uki_off[EFI-A] + uki_size[EFI-A]))" \
  "UKI ROOT-A immediately follows EFI-A (ESP+1)"
# HASH-A must sit immediately after ROOT-A (ESP+2).
assert_eq "${uki_off[HASH-A]}" "$((uki_off[ROOT-A] + uki_size[ROOT-A]))" \
  "UKI HASH-A immediately follows ROOT-A (ESP+2)"

# ROOT-A/HASH-A/PRIVATE sizes are unchanged versus the non-UKI layout.
assert_eq "${uki_size[ROOT-A]}" "${non_uki_size[ROOT-A]}" "ROOT-A size unchanged in UKI layout"
assert_eq "${uki_size[HASH-A]}" "${non_uki_size[HASH-A]}" "HASH-A size unchanged in UKI layout"
assert_eq "${uki_size[PRIVATE]}" "${non_uki_size[PRIVATE]}" "PRIVATE size unchanged in UKI layout"

# Crucially, the total consumed space (and therefore the image size) must be
# identical between the two layouts: merging partitions must not enlarge the image.
uki_total=$((uki_off[DATA-A] + uki_size[DATA-A]))
non_uki_total=$((non_uki_off[DATA-A] + non_uki_size[DATA-A]))
assert_eq "${uki_total}" "${non_uki_total}" "UKI layout total == non-UKI total (image size unchanged)"

###############################################################################
# Test 7: `set_eif_partition_sizes` tight-fit layout.
#
# With rootfs_mib=100 and verity_mib=8:
#   GPT header = 1 MiB
#   ROOT-A     = 100 MiB @ offset 1
#   HASH-A     = 8 MiB   @ offset 101
#   GPT footer = 1 MiB   @ offset 109
#   total      = 110 MiB
###############################################################################
echo "Test 7: set_eif_partition_sizes tight-fit layout"
declare -A eif_size eif_off
set_eif_partition_sizes 100 8 eif_size eif_off

assert_eq "${eif_off[ROOT-A]}"  "1"   "EIF tight ROOT-A offset"
assert_eq "${eif_size[ROOT-A]}" "100" "EIF tight ROOT-A size"
assert_eq "${eif_off[HASH-A]}"  "101" "EIF tight HASH-A offset"
assert_eq "${eif_size[HASH-A]}" "8"   "EIF tight HASH-A size"
# Standard-layout partitions must not leak into the EIF result.
assert_unset eif_size "BIOS"     "EIF layout has no BIOS partition"
assert_unset eif_size "EFI-A"    "EIF layout has no EFI-A partition"
assert_unset eif_size "PRIVATE"  "EIF layout has no PRIVATE partition"
assert_unset eif_size "DATA-A"   "EIF layout has no DATA-A partition"
assert_unset eif_size "RESERVED-A" "EIF layout has no RESERVED-A partition"

###############################################################################
# Test 8: `set_eif_partition_sizes` with explicit target size grows ROOT-A.
#
# rootfs=100, verity=8, target=256 MiB:
#   ROOT-A = 256 - 8 - 2 = 246 MiB, offset 1
#   HASH-A = 8 MiB,               offset 247
#   footer @ 255, total 256
###############################################################################
echo "Test 8: set_eif_partition_sizes with padded target"
declare -A eif_size2 eif_off2
set_eif_partition_sizes 100 8 eif_size2 eif_off2 256

assert_eq "${eif_off2[ROOT-A]}"  "1"   "EIF padded ROOT-A offset"
assert_eq "${eif_size2[ROOT-A]}" "246" "EIF padded ROOT-A size"
assert_eq "${eif_off2[HASH-A]}"  "247" "EIF padded HASH-A offset"
assert_eq "${eif_size2[HASH-A]}" "8"   "EIF padded HASH-A size"

###############################################################################
# Test 9: target equal to the minimum is accepted (edge case).
#
# rootfs=64, verity=4 -> minimum = 64 + 4 + 2 = 70 MiB.
###############################################################################
echo "Test 9: set_eif_partition_sizes target at exact minimum"
declare -A eif_size3 eif_off3
set_eif_partition_sizes 64 4 eif_size3 eif_off3 70
assert_eq "${eif_size3[ROOT-A]}" "64" "EIF exact-min ROOT-A size"
assert_eq "${eif_size3[HASH-A]}" "4"  "EIF exact-min HASH-A size"

###############################################################################
# Test 10: target smaller than the minimum is rejected.
###############################################################################
echo "Test 10: set_eif_partition_sizes rejects target smaller than minimum"
assert_fails "target=50 rejected when minimum is 70" \
  bash -c '
    set -eu -o pipefail
    . "'"${PARTYPLANNER}"'"
    declare -A s o
    set_eif_partition_sizes 64 4 s o 50
  '

###############################################################################
# Test 11: zero / non-numeric arguments are rejected.
###############################################################################
echo "Test 11: set_eif_partition_sizes rejects zero and non-numeric sizes"
assert_fails "rootfs_mib=0 rejected" \
  bash -c '
    set -eu -o pipefail
    . "'"${PARTYPLANNER}"'"
    declare -A s o
    set_eif_partition_sizes 0 4 s o
  '
assert_fails "verity_mib=abc rejected" \
  bash -c '
    set -eu -o pipefail
    . "'"${PARTYPLANNER}"'"
    declare -A s o
    set_eif_partition_sizes 64 abc s o
  '

###############################################################################
echo
echo "Results: ${pass_count} passed, ${fail_count} failed"
if [[ "${fail_count}" -gt 0 ]]; then
  exit 1
fi
