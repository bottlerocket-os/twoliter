#!/usr/bin/env bash
#
# Test how the EFI system partition is populated for UKI images:
#
#   1. `provide_loader_config` in `imghelper` copies the systemd-boot
#      configuration from the ESP staging directory into `\loader\loader.conf`
#      on the FAT-formatted EFI image, byte for byte.
#   2. A missing configuration file is not fatal: `provide_loader_config`
#      warns and returns success, because systemd-boot falls back to its
#      compiled-in defaults and the rest of the ESP is intact.
#   3. `rpm2img` calls `provide_loader_config` only for UKI images, since
#      GRUB does not read `\loader\loader.conf`.
#   4. `rpm2img` still copies the loader binaries into `::/EFI/BOOT`, which is
#      where shim looks for its second stage. The shim package is built with
#      `DEFAULT_LOADER=systemd-boot<arch>.efi`, an unqualified name that shim
#      resolves in its own directory, so the systemd-boot binary has to sit
#      next to the shim rather than in `::/EFI/systemd`.
#
# Complements:
#   - `test_partyplanner.sh`: partition-layout math.
#   - `test_rpm2eif_args.sh`: EIF flag validation.
#   - `test_guest_images_helper.sh`: artifact suffix allowlist.
#
# Run from the repo root:
#   bash twoliter/embedded/tests/test_esp_loader_config.sh

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EMBEDDED_DIR="${SCRIPT_DIR}/.."
IMGHELPER="${EMBEDDED_DIR}/imghelper"
RPM2IMG="${EMBEDDED_DIR}/rpm2img"

for f in "${IMGHELPER}" "${RPM2IMG}"; do
  if [[ ! -f "${f}" ]]; then
    echo "test_esp_loader_config: required file not found: ${f}" >&2
    exit 1
  fi
done

pass_count=0
fail_count=0

pass() { pass_count=$((pass_count + 1)); echo "  ok: $1"; }
fail() { fail_count=$((fail_count + 1)); echo "  FAIL: $1" >&2; }

tmp=$(mktemp -d)
trap 'rm -rf "${tmp}"' EXIT

# Stand in for the FAT32 EFI-A partition image that `rpm2img` builds with
# `mkfs.vfat`. `mformat` is used here because it needs no privileges and comes
# from the same `mtools` package as the `mmd`/`mcopy` calls under test.
make_efi_image() {
  local image
  image="${1:?}"
  dd if=/dev/zero of="${image}" bs=1M count=8 status=none
  mformat -i "${image}" -F -v EFI ::
  mmd -i "${image}" ::/EFI
  mmd -i "${image}" ::/EFI/BOOT
}

# ---------------------------------------------------------------------------
# Test 1: provide_loader_config copies loader.conf to ::/loader unchanged.
# ---------------------------------------------------------------------------
efi_mount="${tmp}/efi_mount"
mkdir -p "${efi_mount}/EFI/BOOT" "${efi_mount}/loader"
cat >"${efi_mount}/loader/loader.conf" <<'EOF'
# systemd-boot configuration
timeout 0
editor no
secure-boot-enroll off
EOF

efi_image="${tmp}/efi.img"
make_efi_image "${efi_image}"

(
  VERSION_ID=x BUILD_ID=x IMAGE_NAME=x VARIANT=x ARCH=x86_64
  # shellcheck disable=SC1090
  . "${IMGHELPER}"
  provide_loader_config "${efi_image}" "${efi_mount}"
) && rc=0 || rc=$?

if [[ "${rc}" -ne 0 ]]; then
  fail "provide_loader_config returned ${rc} with a loader.conf present"
else
  got="${tmp}/loader.conf.roundtrip"
  if mcopy -i "${efi_image}" -o "::/loader/loader.conf" "${got}" 2>/dev/null &&
    cmp -s "${efi_mount}/loader/loader.conf" "${got}"; then
    pass "provide_loader_config copies loader.conf to ::/loader unchanged"
  else
    fail "loader.conf missing from ::/loader or altered in transit"
  fi
fi

# ---------------------------------------------------------------------------
# Test 2: the configuration lands at ::/loader/loader.conf and nowhere else.
# systemd-boot reads only that path, and only from the volume it was loaded
# from, so a copy under ::/EFI/BOOT would be silently ignored.
# ---------------------------------------------------------------------------
if mdir -i "${efi_image}" -b "::/EFI/BOOT" 2>/dev/null | grep -qi "loader.conf"; then
  fail "loader.conf was copied into ::/EFI/BOOT, where systemd-boot never looks"
else
  pass "loader.conf is not placed in ::/EFI/BOOT"
fi

# ---------------------------------------------------------------------------
# Test 3: a missing loader.conf warns but does not fail the image build.
# ---------------------------------------------------------------------------
bare_mount="${tmp}/bare_mount"
mkdir -p "${bare_mount}/EFI/BOOT"
bare_image="${tmp}/bare.img"
make_efi_image "${bare_image}"

out=$(
  VERSION_ID=x BUILD_ID=x IMAGE_NAME=x VARIANT=x ARCH=x86_64 \
    bash -c "
    set -eu -o pipefail
    . '${IMGHELPER}'
    provide_loader_config '${bare_image}' '${bare_mount}'
  " 2>&1
) && rc=0 || rc=$?

if [[ "${rc}" -eq 0 && "${out}" == *"no systemd-boot configuration found"* ]]; then
  pass "provide_loader_config warns and succeeds when loader.conf is absent"
else
  fail "absent loader.conf should warn and succeed; rc=${rc} out: ${out}"
fi

# ---------------------------------------------------------------------------
# Test 4: rpm2img calls provide_loader_config, and only for UKI images.
# ---------------------------------------------------------------------------
if grep -q 'provide_loader_config "${EFI_IMAGE}" "${EFI_MOUNT}"' "${RPM2IMG}"; then
  # The call must sit inside a `UKI_IMAGE == yes` guard; check the nearest
  # preceding conditional.
  guard=$(grep -B5 'provide_loader_config' "${RPM2IMG}" | grep -E 'if \[\[.*UKI_IMAGE' || true)
  if [[ -n "${guard}" && "${guard}" == *'"yes"'* ]]; then
    pass "rpm2img calls provide_loader_config under a UKI_IMAGE guard"
  else
    fail "rpm2img calls provide_loader_config outside a UKI_IMAGE == yes guard"
  fi
else
  fail "rpm2img does not call provide_loader_config with EFI_IMAGE and EFI_MOUNT"
fi

# ---------------------------------------------------------------------------
# Test 5: rpm2img still copies the loader binaries next to the shim, where
# shim's compiled-in DEFAULT_LOADER expects to find systemd-boot.
# ---------------------------------------------------------------------------
if grep -q 'mcopy -i "${EFI_IMAGE}" "${EFI_MOUNT}/EFI/BOOT"/\*\.efi ::/EFI/BOOT' "${RPM2IMG}"; then
  pass "rpm2img copies EFI binaries into ::/EFI/BOOT alongside the shim"
else
  fail 'rpm2img no longer copies ${EFI_MOUNT}/EFI/BOOT/*.efi into ::/EFI/BOOT'
fi

# ---------------------------------------------------------------------------
echo
echo "test_esp_loader_config: ${pass_count} passed, ${fail_count} failed"
[[ "${fail_count}" -eq 0 ]]
