#!/usr/bin/env bash
#
# Test `uki_bootconfig_cmdline` in `imghelper`: the function that hard-codes
# the core kit's systemd and FIPS bootconfig snippets
# (/boot/boot-config.d/20-*, 21-*, 10-fips.conf) as literal UKI cmdline
# tokens, since bootconfig.data is not yet attachable to a UKI's boot chain.
#
# FIPS-ness is derived from the variant name (the `-fips` suffix convention
# used across every FIPS variant under bottlerocket/variants/), not from a
# separate build-arg, so this test exercises that name-based branch directly.
#
# Run from the repo root:
#   bash twoliter/embedded/tests/test_uki_bootconfig.sh

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMGHELPER="${SCRIPT_DIR}/../imghelper"

if [[ ! -f "${IMGHELPER}" ]]; then
  echo "test_uki_bootconfig: imghelper not found at ${IMGHELPER}" >&2
  exit 1
fi

# imghelper's top-level `${VAR:?}` expansions require these to be set before
# it can be sourced, even though this test only exercises
# `uki_bootconfig_cmdline`.
IMAGE_NAME=x VARIANT=x ARCH=x86_64 VERSION_ID=x BUILD_ID=x
export IMAGE_NAME VARIANT ARCH VERSION_ID BUILD_ID

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
    pass "${name}"
  else
    fail "${name}: expected '${expected}' got '${actual}'"
  fi
}

# Assert that a space-separated token list contains every expected token.
assert_contains_tokens() {
  local actual name token
  actual="$1"
  name="$2"
  shift 2
  for token in "$@"; do
    if [[ " ${actual} " != *" ${token} "* ]]; then
      fail "${name}: expected token '${token}' missing from '${actual}'"
      return
    fi
  done
  pass "${name}"
}

echo "Test 1: UKI + FIPS variant includes systemd and FIPS tokens"
out="$(uki_bootconfig_cmdline "aws-k8s-1.35-fips")"
assert_contains_tokens "${out}" "fips variant: systemd tokens present" \
  "SYSTEMD_CGROUP_ENABLE_LEGACY_FORCE=1" "SYSTEMD_DEFAULT_MOUNT_RATE_LIMIT_BURST=25" "module_blacklist=i8042"
assert_contains_tokens "${out}" "fips variant: FIPS tokens present" \
  "fips=1" "init.systemd.unit=fipscheck.target"

echo
echo "Test 1b: UKI + FIPS variant with an extra flavor segment (nvidia-fips)"
out="$(uki_bootconfig_cmdline "aws-k8s-1.34-nvidia-fips")"
assert_contains_tokens "${out}" "nvidia-fips variant: systemd tokens present" \
  "SYSTEMD_CGROUP_ENABLE_LEGACY_FORCE=1" "SYSTEMD_DEFAULT_MOUNT_RATE_LIMIT_BURST=25" "module_blacklist=i8042"
assert_contains_tokens "${out}" "nvidia-fips variant: FIPS tokens present" \
  "fips=1" "init.systemd.unit=fipscheck.target"

echo
echo "Test 2: UKI + non-FIPS variant includes systemd tokens only"
out="$(uki_bootconfig_cmdline "aws-k8s-1.35")"
assert_contains_tokens "${out}" "non-fips variant: systemd tokens present" \
  "SYSTEMD_CGROUP_ENABLE_LEGACY_FORCE=1" "SYSTEMD_DEFAULT_MOUNT_RATE_LIMIT_BURST=25" "module_blacklist=i8042"
if [[ " ${out} " == *" fips=1 "* ]] || [[ " ${out} " == *" init.systemd.unit=fipscheck.target "* ]]; then
  fail "non-fips variant: FIPS tokens must be absent, got '${out}'"
else
  pass "non-fips variant: FIPS tokens absent"
fi

echo
echo "Test 2b: a variant merely containing 'fips' mid-string is not FIPS"
# Guards against a naive substring match: FIPS-ness is a trailing dash-token,
# not merely the presence of the substring 'fips' anywhere in the name.
out="$(uki_bootconfig_cmdline "aws-fips-k8s-1.35")"
if [[ " ${out} " == *" fips=1 "* ]]; then
  fail "mid-string 'fips' incorrectly treated as a FIPS variant, got '${out}'"
else
  pass "mid-string 'fips' correctly not treated as a FIPS variant"
fi

echo
echo "Test 3: non-UKI images never call uki_bootconfig_cmdline (values absent)"
# rpm2img only calls uki_bootconfig_cmdline inside the `UKI_IMAGE == yes`
# branch; a non-UKI build never invokes it, so uki_bootconfig is never
# populated there. We assert the source-level guard directly, since
# rpm2img itself is not practical to run end-to-end in a unit test (it
# performs real partitioning, RPM installs, and image assembly first).
RPM2IMG="${SCRIPT_DIR}/../rpm2img"
if [[ ! -f "${RPM2IMG}" ]]; then
  echo "test_uki_bootconfig: rpm2img not found at ${RPM2IMG}" >&2
  exit 1
fi
if grep -qE '^\s*if \[\[ "\$\{UKI_IMAGE\}" == "yes" \]\]; then\s*$' "${RPM2IMG}"; then
  # Extract the UKI-image conditional block and confirm the
  # uki_bootconfig_cmdline call lives inside it.
  uki_block="$(awk '/if \[\[ "\$\{UKI_IMAGE\}" == "yes" \]\]; then/,/^fi$/' "${RPM2IMG}")"
  if [[ "${uki_block}" == *"uki_bootconfig_cmdline"* ]]; then
    pass "uki_bootconfig_cmdline is only invoked inside the UKI_IMAGE=yes branch"
  else
    fail "uki_bootconfig_cmdline call not found inside the UKI_IMAGE=yes branch"
  fi
else
  fail "could not locate the UKI_IMAGE=yes conditional in rpm2img"
fi

echo
echo "Results: ${pass_count} passed, ${fail_count} failed"
if [[ "${fail_count}" -gt 0 ]]; then
  exit 1
fi
