#!/usr/bin/env bash
#
# Test the drift-detection contract between `imghelper` and
# `guest-images-helper`:
#
#   1. `imghelper` publishes `IMAGE_ARTIFACT_SUFFIXES` as the single source
#      of truth for artifact suffixes an image build emits.
#   2. `compress_image` in `imghelper` rejects any suffix not in that list,
#      so a new format cannot be added at a call site without also being
#      declared.
#   3. Every literal `compress_image "<ext>"` invocation in `rpm2img` and
#      `img2img` uses a suffix that is in `IMAGE_ARTIFACT_SUFFIXES`.
#   4. `guest-images-helper` refuses to load unless `imghelper` has been
#      sourced first (fails loudly rather than silently skipping artifact
#      types).
#   5. `copy_guest_image_artifacts` copies every artifact whose suffix is
#      in the allowlist, and rejects sidecar metadata (`*.json`, SBOM,
#      inventory).
#
# Complements:
#   - `test_partyplanner.sh`: partition-layout math.
#   - `test_rpm2eif_args.sh`: EIF flag validation.
#
# Run from the repo root:
#   bash twoliter/embedded/tests/test_guest_images_helper.sh

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EMBEDDED_DIR="${SCRIPT_DIR}/.."
IMGHELPER="${EMBEDDED_DIR}/imghelper"
GUEST_HELPER="${EMBEDDED_DIR}/guest-images-helper"
RPM2IMG="${EMBEDDED_DIR}/rpm2img"
IMG2IMG="${EMBEDDED_DIR}/img2img"

for f in "${IMGHELPER}" "${GUEST_HELPER}" "${RPM2IMG}" "${IMG2IMG}"; do
  if [[ ! -f "${f}" ]]; then
    echo "test_guest_images_helper: required file not found: ${f}" >&2
    exit 1
  fi
done

pass_count=0
fail_count=0

pass() { pass_count=$((pass_count + 1)); echo "  ok: $1"; }
fail() { fail_count=$((fail_count + 1)); echo "  FAIL: $1" >&2; }

# ---------------------------------------------------------------------------
# Test 1: imghelper defines IMAGE_ARTIFACT_SUFFIXES with the expected members.
# ---------------------------------------------------------------------------
(
  # Provide the env vars imghelper's top-level `${X:?}` checks require.
  VERSION_ID=x BUILD_ID=x IMAGE_NAME=x VARIANT=x ARCH=x86_64
  # shellcheck disable=SC1090
  . "${IMGHELPER}"
  declare -p IMAGE_ARTIFACT_SUFFIXES >/dev/null 2>&1 || {
    echo "IMAGE_ARTIFACT_SUFFIXES not declared" >&2; exit 1;
  }
  # At minimum, the canonical set. New entries are allowed; missing entries fail.
  # `eif` is included because EIF guest variants ship a `.eif` sidecar that
  # host repack needs to be able to enumerate and resign.
  required=("img.lz4" "qcow2" "vmdk" "ova" "ext4.lz4" "verity.lz4" "eif")
  for want in "${required[@]}"; do
    found=no
    for have in "${IMAGE_ARTIFACT_SUFFIXES[@]}"; do
      [[ "${have}" == "${want}" ]] && { found=yes; break; }
    done
    [[ "${found}" == "yes" ]] || { echo "missing suffix: ${want}" >&2; exit 1; }
  done
) && pass "imghelper defines IMAGE_ARTIFACT_SUFFIXES with canonical members" \
   || fail "imghelper does not publish the expected IMAGE_ARTIFACT_SUFFIXES set"

# ---------------------------------------------------------------------------
# Test 2: compress_image rejects an unknown extension with a clear error.
# ---------------------------------------------------------------------------
out=$(
  VERSION_ID=x BUILD_ID=x IMAGE_NAME=x VARIANT=x ARCH=x86_64 \
  bash -c "
    set -eu -o pipefail
    . '${IMGHELPER}'
    compress_image 'made.up.format' 'os_image' /tmp
  " 2>&1
) || true
if [[ "${out}" == *"not in IMAGE_ARTIFACT_SUFFIXES"* ]]; then
  pass "compress_image rejects an unknown extension"
else
  fail "compress_image should reject unknown extension; got: ${out}"
fi

# ---------------------------------------------------------------------------
# Test 3: every `compress_image "<ext>"` and `symlink_image "<ext>"` literal
# in rpm2img and img2img is a member of IMAGE_ARTIFACT_SUFFIXES.
# ---------------------------------------------------------------------------
(
  VERSION_ID=x BUILD_ID=x IMAGE_NAME=x VARIANT=x ARCH=x86_64
  # shellcheck disable=SC1090
  . "${IMGHELPER}"
  # Collect literals: `compress_image "img.lz4" ...` -> `img.lz4`.
  mapfile -t used < <(
    grep -hoE '(compress_image|symlink_image) "[^"]+"' "${RPM2IMG}" "${IMG2IMG}" \
      | sed -E 's/^(compress_image|symlink_image) "//; s/"$//' \
      | sort -u
  )
  [[ "${#used[@]}" -gt 0 ]] || {
    echo "no compress_image/symlink_image call sites found; grep likely broken" >&2; exit 1;
  }
  for ext in "${used[@]}"; do
    found=no
    for suffix in "${IMAGE_ARTIFACT_SUFFIXES[@]}"; do
      [[ "${suffix}" == "${ext}" ]] && { found=yes; break; }
    done
    [[ "${found}" == "yes" ]] || {
      echo "call-site suffix '${ext}' is not declared in IMAGE_ARTIFACT_SUFFIXES" >&2
      exit 1
    }
  done
) && pass "every compress_image/symlink_image call site uses a declared suffix" \
   || fail "found compress_image/symlink_image invocation with undeclared suffix"

# ---------------------------------------------------------------------------
# Test 4: guest-images-helper refuses to load without imghelper first.
# ---------------------------------------------------------------------------
out=$(
  bash -c "
    set -eu -o pipefail
    unset IMAGE_ARTIFACT_SUFFIXES
    . '${GUEST_HELPER}'
  " 2>&1
) || true
if [[ "${out}" == *"IMAGE_ARTIFACT_SUFFIXES is unset"* ]]; then
  pass "guest-images-helper fails loudly without imghelper"
else
  fail "guest-images-helper should refuse to load; got: ${out}"
fi

# ---------------------------------------------------------------------------
# Test 5: copy_guest_image_artifacts copies allowlisted artifacts and
# rejects sidecar metadata.
# ---------------------------------------------------------------------------
tmp=$(mktemp -d)
trap 'rm -rf "${tmp}"' EXIT
src="${tmp}/src"; dst="${tmp}/dst"
mkdir -p "${src}" "${dst}"

# Two bootable artifacts (one per allowlisted suffix), one stable-name symlink,
# EIF artifacts (eif, disk.img, kernel), and three sidecar-metadata files that
# must be rejected.
touch "${src}/bottlerocket-1.0.0.img.lz4"
touch "${src}/bottlerocket-1.0.0.ext4.lz4"
touch "${src}/bottlerocket-1.0.0.eif"
ln -s "bottlerocket-1.0.0.img.lz4" "${src}/os_image.img.lz4"
touch "${src}/bottlerocket-1.0.0.eif"
touch "${src}/bottlerocket-1.0.0-disk.img"
touch "${src}/bottlerocket-1.0.0-kernel"
ln -s "bottlerocket-1.0.0.eif" "${src}/latest.eif"
touch "${src}/application-inventory.json"
touch "${src}/spdx-sbom.json"
touch "${src}/artifact-metadata.json"

(
  VERSION_ID=x BUILD_ID=x IMAGE_NAME=x VARIANT=x ARCH=x86_64
  # shellcheck disable=SC1090
  . "${IMGHELPER}"
  # shellcheck disable=SC1090
  . "${GUEST_HELPER}"
  copy_guest_image_artifacts "${src}" "${dst}"
) && rc=0 || rc=$?

if [[ "${rc}" -ne 0 ]]; then
  fail "copy_guest_image_artifacts returned ${rc} on a valid source dir"
else
  # Verify allowlisted artifacts landed at the destination.
  wanted=("bottlerocket-1.0.0.img.lz4" "bottlerocket-1.0.0.ext4.lz4" "os_image.img.lz4"
          "bottlerocket-1.0.0.eif" "bottlerocket-1.0.0-disk.img" "bottlerocket-1.0.0-kernel"
          "latest.eif")
  missing=""
  for f in "${wanted[@]}"; do
    [[ -e "${dst}/${f}" ]] || missing+=" ${f}"
  done
  # Verify sidecar metadata was NOT copied.
  unwanted=("application-inventory.json" "spdx-sbom.json" "artifact-metadata.json")
  leaked=""
  for f in "${unwanted[@]}"; do
    [[ -e "${dst}/${f}" ]] && leaked+=" ${f}"
  done
  # Verify the symlink was preserved as a symlink (not dereferenced).
  sym_ok="yes"
  [[ -L "${dst}/os_image.img.lz4" ]] || sym_ok="no"

  if [[ -z "${missing}" && -z "${leaked}" && "${sym_ok}" == "yes" ]]; then
    pass "copy_guest_image_artifacts copies allowlisted files, drops metadata, preserves symlinks"
  else
    fail "copy_guest_image_artifacts had issues:${missing:+ missing:${missing}}${leaked:+ leaked:${leaked}}${sym_ok:+ symlink_ok=${sym_ok}}"
  fi
fi

# ---------------------------------------------------------------------------
# Test 6: copy_guest_image_artifacts fails if the source is empty of
# allowlisted artifacts (only metadata present).
# ---------------------------------------------------------------------------
empty_src="${tmp}/empty_src"; empty_dst="${tmp}/empty_dst"
mkdir -p "${empty_src}" "${empty_dst}"
touch "${empty_src}/application-inventory.json"
touch "${empty_src}/spdx-sbom.json"

(
  VERSION_ID=x BUILD_ID=x IMAGE_NAME=x VARIANT=x ARCH=x86_64
  # shellcheck disable=SC1090
  . "${IMGHELPER}"
  # shellcheck disable=SC1090
  . "${GUEST_HELPER}"
  copy_guest_image_artifacts "${empty_src}" "${empty_dst}"
) && rc=0 || rc=$?

if [[ "${rc}" -ne 0 ]]; then
  pass "copy_guest_image_artifacts fails when no allowlisted artifacts present"
else
  fail "copy_guest_image_artifacts should have failed on metadata-only source"
fi

# ---------------------------------------------------------------------------
echo
echo "test_guest_images_helper: ${pass_count} passed, ${fail_count} failed"
[[ "${fail_count}" -eq 0 ]]
