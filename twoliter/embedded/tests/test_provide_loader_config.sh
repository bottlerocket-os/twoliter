#!/usr/bin/env bash
#
# Test `provide_loader_config` in `imghelper`:
#
#   When `loader/loader.conf` is missing from the ESP staging mount, the
#   function must fail hard (non-zero exit) with an actionable error naming
#   the searched path, instead of silently continuing with systemd-boot's
#   compiled-in defaults.
#
# Run from the repo root:
#   bash twoliter/embedded/tests/test_provide_loader_config.sh

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EMBEDDED_DIR="${SCRIPT_DIR}/.."
IMGHELPER="${EMBEDDED_DIR}/imghelper"

if [[ ! -f "${IMGHELPER}" ]]; then
  echo "test_provide_loader_config: required file not found: ${IMGHELPER}" >&2
  exit 1
fi

pass_count=0
fail_count=0

pass() { pass_count=$((pass_count + 1)); echo "  ok: $1"; }
fail() { fail_count=$((fail_count + 1)); echo "  FAIL: $1" >&2; }

tmp=$(mktemp -d)
trap 'rm -rf "${tmp}"' EXIT

# ---------------------------------------------------------------------------
# Test: missing loader.conf fails hard with an actionable message that
# names the searched path.
# ---------------------------------------------------------------------------
efi_mount="${tmp}/efi_mount"
mkdir -p "${efi_mount}"
efi_image="${tmp}/efi.img"
loader_config="${efi_mount}/loader/loader.conf"

out=$(
  VERSION_ID=x BUILD_ID=x IMAGE_NAME=x VARIANT=x ARCH=x86_64 \
  bash -c "
    set -eu -o pipefail
    . '${IMGHELPER}'
    provide_loader_config '${efi_image}' '${efi_mount}'
  " 2>&1
) && rc=0 || rc=$?

if [[ "${rc}" -eq 0 ]]; then
  fail "provide_loader_config should fail when loader.conf is missing, got exit 0"
elif [[ "${out}" != *"${loader_config}"* ]]; then
  fail "error message should name the searched path '${loader_config}'; got: ${out}"
else
  pass "provide_loader_config fails hard and names the searched path when loader.conf is missing"
fi

echo
echo "test_provide_loader_config: ${pass_count} passed, ${fail_count} failed"
[[ "${fail_count}" -eq 0 ]]
