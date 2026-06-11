#!/usr/bin/env bash
#
# Test the argument-validation half of `rpm2eif`: check that the EIF flag
# allowlist rejects incompatible feature combinations with clear errors,
# that `--os-image-size-gib` is validated as an integer, and that the
# legal-flag path proceeds past validation (fails later on the missing
# RPM directory, which is the point at which we stop caring).
#
# Complements:
#   - `test_partyplanner.sh`: layout math (`set_eif_partition_sizes`).
#   - Rust-side tests in `tools/buildsys/src/manifest.rs`: feature
#     validation as seen by the higher-level build orchestrator.
#
# Run from the repo root:
#   bash twoliter/embedded/tests/test_rpm2eif_args.sh

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RPM2EIF="${SCRIPT_DIR}/../rpm2eif"

if [[ ! -f "${RPM2EIF}" ]]; then
  echo "test_rpm2eif_args: rpm2eif not found at ${RPM2EIF}" >&2
  exit 1
fi

pass_count=0
fail_count=0

pass() { pass_count=$((pass_count + 1)); echo "  ok: $1"; }
fail() { fail_count=$((fail_count + 1)); echo "  FAIL: $1" >&2; }

# Run rpm2eif with the given flags and capture stdout+stderr and exit code.
# We set the minimum required env vars so imghelper's initial `${X:?}`
# checks succeed and we reach the flag-validation logic. `--package-dir`
# points at a non-existent path so that if validation passes, the script
# fails soon after on the missing RPMs and we can distinguish
# "validation-error exit" from "post-validation exit" via the error text.
run_rpm2eif() {
  local out
  out=$(
    VERSION_ID=x BUILD_ID=x IMAGE_NAME=x VARIANT=x VARIANT_NAME=x ARCH=x86_64 \
    bash "${RPM2EIF}" "$@" 2>&1
  ) || true
  printf "%s" "${out}"
}

# Assert that running rpm2eif with the given flags produces output that
# contains the expected substring AND is a validation-rejection (never
# reaches "=== Installing RPMs ===").
assert_rejects_with() {
  local name="$1"; shift
  local needle="$1"; shift
  local out
  out=$(run_rpm2eif "$@")
  if [[ "${out}" == *"${needle}"* ]] && [[ "${out}" != *"=== Installing RPMs ==="* ]]; then
    pass "${name}"
  else
    fail "${name}: expected rejection containing '${needle}', got: ${out}"
  fi
}

# Assert that validation *passes* -- the script reaches its RPM install
# step and then fails on the missing package dir. This confirms the
# combination is not gated by the EIF-flag allowlist.
assert_passes_validation() {
  local name="$1"; shift
  local out
  out=$(run_rpm2eif "$@")
  if [[ "${out}" == *"=== Installing RPMs ==="* ]]; then
    pass "${name}"
  else
    fail "${name}: validation should have passed, got: ${out}"
  fi
}

BASE=(
  --package-dir=/tmp/rpm2eif-test-no-such-dir
  --output-dir=/tmp/rpm2eif-test-out
  --external-kits-path=/tmp/rpm2eif-test-kits
  --with-first-party-stack=no
)

echo "Test 1: legal minimal flags pass validation"
assert_passes_validation "legal minimal flags" "${BASE[@]}"

echo
echo "Test 2: reject uefi-secure-boot=yes"
assert_rejects_with "uefi-secure-boot=yes rejected" "uefi-secure-boot" \
  "${BASE[@]}" --with-uefi-secure-boot=yes

echo
echo "Test 3: reject in-place-updates=yes"
assert_rejects_with "in-place-updates=yes rejected" "in-place-updates" \
  "${BASE[@]}" --with-in-place-updates=yes

echo
echo "Test 4: reject encrypted-storage=yes"
assert_rejects_with "encrypted-storage=yes rejected" "encrypted-storage" \
  "${BASE[@]}" --with-encrypted-storage=yes

echo
echo "Test 5: reject xfs-data-partition=yes"
assert_rejects_with "xfs-data-partition=yes rejected" "xfs-data-partition" \
  "${BASE[@]}" --with-xfs-data-partition=yes

echo
echo "Test 6: reject first-party-stack=yes"
assert_rejects_with "first-party-stack=yes rejected" "first-party-stack" \
  "${BASE[@]/--with-first-party-stack=no/--with-first-party-stack=yes}"

echo
echo "Test 7: reject non-yes erofs-root-partition"
assert_rejects_with "erofs-root-partition=no rejected" "erofs-root-partition" \
  "${BASE[@]}" --with-erofs-root-partition=no

echo
echo "Test 8: accept erofs-root-partition=yes (parity with rpm2img default)"
assert_passes_validation "erofs-root-partition=yes accepted" \
  "${BASE[@]}" --with-erofs-root-partition=yes

echo
echo "Test 9: reject non-numeric --os-image-size-gib"
assert_rejects_with "os-image-size-gib=abc rejected" "not a non-negative integer" \
  "${BASE[@]}" --os-image-size-gib=abc

echo
echo "Test 10: accept --os-image-size-gib=0 (auto-size)"
assert_passes_validation "os-image-size-gib=0 accepted" \
  "${BASE[@]}" --os-image-size-gib=0

echo
echo "Test 11: accept --os-image-size-gib=2"
assert_passes_validation "os-image-size-gib=2 accepted" \
  "${BASE[@]}" --os-image-size-gib=2

echo
echo "Test 11a: reject negative --os-image-size-gib"
# A leading `-` is not part of the accepted integer regex, so a negative
# value must be rejected the same way a non-numeric one is.
assert_rejects_with "os-image-size-gib=-1 rejected" "not a non-negative integer" \
  "${BASE[@]}" --os-image-size-gib=-1

echo
echo "Test 11b: reject decimal --os-image-size-gib"
# A positive but non-integer value (e.g. 1.5) must also be rejected -- the
# validator is strict about integer form, not just sign.
assert_rejects_with "os-image-size-gib=1.5 rejected" "not a non-negative integer" \
  "${BASE[@]}" --os-image-size-gib=1.5

echo
echo "Test 12: reject unknown argument"
assert_rejects_with "unknown arg rejected" "Unknown argument" \
  "${BASE[@]}" --this-flag-does-not-exist=1

echo
echo "Test 13: all conflicts reported in a single run"
# Regression guard: rpm2eif should report every failing flag, not stop at
# the first. The user should see the full picture in one build cycle.
out=$(run_rpm2eif \
  "${BASE[@]/--with-first-party-stack=no/--with-first-party-stack=yes}" \
  --with-uefi-secure-boot=yes \
  --with-in-place-updates=yes \
  --with-encrypted-storage=yes)
missing=()
for needle in \
    "uefi-secure-boot" \
    "in-place-updates" \
    "encrypted-storage" \
    "first-party-stack"
do
  if [[ "${out}" != *"${needle}"* ]]; then
    missing+=("${needle}")
  fi
done
if (( ${#missing[@]} == 0 )); then
  pass "all four conflicts reported at once"
else
  fail "missing conflict messages: ${missing[*]}; output: ${out}"
fi

echo
echo "Results: ${pass_count} passed, ${fail_count} failed"
if [[ "${fail_count}" -gt 0 ]]; then
  exit 1
fi
