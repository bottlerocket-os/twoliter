#!/bin/bash
# Generates attributions for dependencies of Twoliter
# Meant to be run from Bottlerocket's SDK container:
# https://github.com/bottlerocket-os/bottlerocket-sdk

# See the "attribution" target in the project Makefile.

set -eo pipefail

LICENSEDIR=/tmp/twoliter-attributions

# Use the toolchain installed via `Dockerfile.attribution`
export HOME="/home/attribution-creator"
source ~/.cargo/env

# Source code is mounted to /src
# rustup will automatically use the toolchain in rust-toolchain.toml
cd /src

# =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
echo "Clarifying crate dependency licenses..."
/usr/libexec/tools/bottlerocket-license-scan \
    --clarify /src/clarify.toml \
    --spdx-data /usr/libexec/tools/spdx-data \
    --out-dir ${LICENSEDIR}/vendor \
    cargo --locked Cargo.toml

# =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
# go-containerregistry
#
# We ship an official prebuilt `krane` binary from the go-containerregistry
# release, but for attribution we still fetch the matching source tarball
# (pinned by SHA512 in `tools/krane/hashes/source`) so we can vendor its Go
# dependencies and scan their licenses.
pushd /src/tools/krane
../build-cache-fetch hashes/source
TARBALL=$(grep -oP '\(\K[^\)]*' hashes/source)
GO_CONTAINERREGISTRY_UNPACK_DIR=$(mktemp -d)
# The upstream release tarball (goreleaser output) has files at the top
# level, unlike GitHub's auto-generated archive which wraps them in a
# `go-containerregistry-<version>/` directory, so no components to strip.
tar -xvf "${TARBALL}" -C "${GO_CONTAINERREGISTRY_UNPACK_DIR}"

pushd "${GO_CONTAINERREGISTRY_UNPACK_DIR}/cmd/krane"
go mod vendor
popd

/usr/libexec/tools/bottlerocket-license-scan \
    --clarify /src/clarify.toml \
    --spdx-data /usr/libexec/tools/spdx-data \
    --out-dir ${LICENSEDIR}/krane \
    go-vendor "${GO_CONTAINERREGISTRY_UNPACK_DIR}/cmd/krane/vendor"
popd

# =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
# Twoliter licenses
cp /src/COPYRIGHT /src/LICENSE-MIT /src/LICENSE-APACHE \
    ${LICENSEDIR}/

pushd "$(dirname ${LICENSEDIR})"
tar czf /src/twoliter-attributions.tar.gz "$(basename ${LICENSEDIR})"
popd
