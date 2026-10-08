#!/usr/bin/env bash
# Exercise the actual rpm2img bootconfig/CLI wiring with SDK-produced fixtures.
set -euo pipefail

embedded=$1
fixtures=$2
ukisys=$3
work=$4

for mode in plain fips; do
  case_dir="${work}/${mode}"
  mkdir -p "${case_dir}/boot" "${case_dir}/config" "${case_dir}/root/lib/modules/6.18.48"
  touch "${case_dir}/root/linuxx64.efi.stub"
  ln -s "${ukisys}" "${case_dir}/ukisys"
  fixture=empty
  parameters="console=ttyS0"
  if [[ "${mode}" == fips ]]; then
    fixture=package
    parameters+=" fips=1"
    mkdir "${case_dir}/boot/boot-config.d"
    cp "${fixtures}/package.conf" "${case_dir}/boot/boot-config.d/package.conf"
  fi
  # The SDK binary encoder is outside this test. Check its exact input and use
  # its checked-in output; the real ukisys executable consumes that output.
  cat >"${case_dir}/rpm2img" <<'SCRIPT'
set -euo pipefail
BOOT_MOUNT=$1
BOOTCONFIG_DIR=$2
ROOT_MOUNT=$3
fixtures=$4
fixture=$5
KERNEL_PARAMETERS=$6
BOOTCONFIG_INPUT="${BOOTCONFIG_DIR}/bootconfig.in"
UKI_IMAGE=yes
EPHEMERAL_ENCRYPTION_KEYS=yes
DM_VERITY_ROOT=("root,,,ro,0 123 verity")
bootconfig() {
  [[ "$1" == -a && "$2" == "${BOOTCONFIG_INPUT}" ]]
  if [[ "${fixture}" == package ]]; then
    sort "${fixtures}/package.conf" | cmp - "$2"
  else
    cmp "${fixtures}/empty.conf" "$2"
  fi
  cp "${fixtures}/${fixture}.data" "$3"
}
SCRIPT
  # Keep the production fragment, including input generation and CLI arguments.
  # Close the UKI branch before image assembly starts.
  sed -n '/^# Combine any bootconfig/,/^  # Create EFI\/Linux/{
    /^  # Create EFI\/Linux/d
    p
  }' "${embedded}/rpm2img" >>"${case_dir}/rpm2img"
  echo 'fi' >>"${case_dir}/rpm2img"
  bash "${case_dir}/rpm2img" "${case_dir}/boot" "${case_dir}/config" \
    "${case_dir}/root" "${fixtures}" "${fixture}" "${parameters}"
  [[ -s "${case_dir}/config/bootconfig.data" ]]
  actual=$(cat "${case_dir}/config/cmdline")
  expected="${parameters} root=/dev/dm-0 rootwait ro raid=noautodetect random.trust_cpu=on selinux=1 enforcing=1 dm-mod.create=\"root,,,ro,0 123 verity\" -- "
  if [[ "${mode}" == fips ]]; then
    expected="SAMPLE_PACKAGE=enabled console=tty0 console=ttyS0 empty= flag message=\"two words\" ${expected}systemd.unit=fipscheck.target "
  fi
  expected+="systemd.log_target=journal-or-kmsg systemd.log_color=0 systemd.show_status=true"
  if [[ "${actual}" != "${expected}" ]]; then
    printf 'Unexpected %s command line:\n%s\nExpected:\n%s\n' "${mode}" "${actual}" "${expected}" >&2
    exit 1
  fi
done
