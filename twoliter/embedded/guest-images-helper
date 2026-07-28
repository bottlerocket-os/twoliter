#!/usr/bin/env bash
# Helpers for embedding guest-variant disk images into a host variant's rootfs.
#
# This file is sourced by `rpm2img` and by the buildsys integration tests. It is
# safe to source on its own: it defines variables/functions only and performs no
# destructive operations. Do not add top-level side effects here.
#
# The set of embeddable artifact suffixes lives in `imghelper` as
# `IMAGE_ARTIFACT_SUFFIXES`, which is also the allowlist enforced by
# `compress_image`. This helper expects `imghelper` to have been sourced
# already (which is what `rpm2img` and the tests do) and fails loudly
# otherwise, so we never silently miss a new artifact type.
if [[ -z "${IMAGE_ARTIFACT_SUFFIXES+x}" ]]; then
  echo "guest-images-helper: IMAGE_ARTIFACT_SUFFIXES is unset; source imghelper first" >&2
  exit 1
fi

# Copy bootable image artifacts (and their stable-name symlinks) from `${1}`
# into `${2}`, rejecting build metadata such as `*.json` and `*-sbom-*`.
#
# Returns 0 on success, 1 if no matching artifacts were found. Robust to spaces
# in filenames; preserves symlinks so that e.g.
# `os_image.img.lz4 -> bottlerocket-…img.lz4` continues to resolve at the
# destination.
copy_guest_image_artifacts() {
  local src="${1}"
  local dst="${2}"
  local find_args=( "${src}" -maxdepth 1 \( -type f -o -type l \) \( -false )
  local suffix
  for suffix in "${IMAGE_ARTIFACT_SUFFIXES[@]}"; do
    find_args+=( -o -name "*.${suffix}" )
  done
  # EIF artifacts use hyphen-separated names (`*-disk.img`, `*-kernel`) that
  # cannot be expressed as dot-suffix patterns. Include them via the separate
  # globs array when it is defined (it lives in `imghelper` alongside the
  # suffixes array).
  local glob
  if [[ -n "${IMAGE_ARTIFACT_GLOBS+x}" ]]; then
    for glob in "${IMAGE_ARTIFACT_GLOBS[@]}"; do
      find_args+=( -o -name "${glob}" )
    done
  fi
  find_args+=( \) -print0 )
  local copied=0 file
  while IFS= read -r -d '' file; do
    if cp -a "${file}" "${dst}/"; then
      copied=$((copied + 1))
    else
      echo "guest-images: failed to copy '${file}' to '${dst}/'" >&2
      return 1
    fi
  done < <(find "${find_args[@]}")
  [ "${copied}" -gt 0 ]
}
