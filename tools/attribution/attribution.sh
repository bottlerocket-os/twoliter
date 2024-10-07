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
pushd /src/tools/krane
../build-cache-fetch hashes/crane
TARBALL=$(grep -oP '\(\K[^\)]*' hashes/crane)
GO_CONTAINERREGISTRY_UNPACK_DIR=$(mktemp -d)
tar --strip-components=1 -xvf "${TARBALL}" -C "${GO_CONTAINERREGISTRY_UNPACK_DIR}"

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
# cargo-make (we currently use cargo-make from the SDK, but will ship it in Twoliter in the future)
echo "Clarifying bottlerocket-sdk & dependency licenses..."
mkdir -p ${LICENSEDIR}/bottlerocket-sdk/
cp -r /usr/share/licenses/cargo-make \
    ${LICENSEDIR}/bottlerocket-sdk/

# =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=  =^.^=
# Twoliter licenses
cp /src/COPYRIGHT /src/LICENSE-MIT /src/LICENSE-APACHE \
    ${LICENSEDIR}/

pushd "$(dirname ${LICENSEDIR})"
tar czf /src/twoliter-attributions.tar.gz "$(basename ${LICENSEDIR})"
popd
