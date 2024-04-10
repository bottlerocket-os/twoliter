#!/usr/bin/env bash

# The Alpha milestone (i.e. preceding Beta) represents a version of Twoliter that can build a
# customized variant before Kits have been implemented. See:
# - https://github.com/bottlerocket-os/twoliter/issues/74
# - https://github.com/bottlerocket-os/twoliter/issues/56
#
# This script builds a bottlerocket variant, then copies certain contents of the Bottlerocket build
# directory into a layer added to the SDK. Twoliter's build variant command will expect these to be
# available in the SDK at `/twoliter/alpha` until Kits have been implemented.

# The directory this script is located in.
script_dir=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

#
# Common error handling
#

exit_trap_cmds=()

on_exit() {
    exit_trap_cmds+=( "$1" )
}

run_exit_trap_cmds() {
    for cmd in "${exit_trap_cmds[@]}"; do
        eval "${cmd}"
    done
}

trap run_exit_trap_cmds EXIT

warn() {
    >&2 echo "Warning: $*"
}

bail() {
    if [[ $# -gt 0 ]]; then
        >&2 echo "Error: $*"
    fi
    exit 1
}

usage() {
    cat <<EOF

Usage:

    --bottlerocket-dir            REQUIRED: The directory of the Bottlerocket checkout that we will
                                  build.

    --variant                     OPTIONAL: The Bottlerocket variant that we will build. Defaults
                                  to 'aws-dev'.

    --sdk-version                 OPTIONAL: The version of the Bottlerocket SDK to use when building
                                  packages and to use as the base for the Twoliter alpha-sdk. For
                                  example if the SDK version is v0.50.0 then --sdk-version should be
                                  the same (i.e. with the v prefix). When not specified, this will
                                  be taken from Bottlerocket's Twoliter.toml file.

    --sdk-name                    OPTIONAL: The name of the SDK. Defaults to 'bottlerocket-sdk'


    --sdk-registry                OPTIONAL: The namespace or docker registry. Defaults to
                                  'public.ecr.aws/bottlerocket'.

    --alpha-name                  OPTIONAL: The name of the Twoliter alpha SDK container. Defaults
                                  to 'twoliter-alpha-sdk'.

    --alpha-registry              REQUIRED: The registry to which the resultant alpha SDK images
                                  will be pushed.

    --alpha-version               REQUIRED: The version the alpha SDK should be tagged with. In
                                  practice this should match the Bottlerocket version of the
                                  packages being built.

    --skip-clean                  OPTIONAL: To speed things up, you can skip running cargo make
                                  clean.

    -h, --help                    Show this help text

EOF
}

usage_error() {
    >&2 usage
    bail "$1"
}

#
# Parse arguments
#

while [[ $# -gt 0 ]]; do
    case $1 in
        --bottlerocket-dir)
            shift; bottlerocket_dir=$1 ;;
        --variant)
            shift; variant=$1 ;;
        --sdk-version)
            shift; sdk_version=$1 ;;
        --sdk-name)
            shift; sdk_name=$1 ;;
        --sdk-registry)
            shift; sdk_registry=$1 ;;
        --alpha-registry)
            shift; alpha_registry=$1 ;;
        --alpha-name)
            shift; alpha_name=$1 ;;
        --alpha-version)
            shift; alpha_version=$1 ;;
        --skip-clean)
            shift; skip_clean="true" ;;
        -h|--help)
            usage; exit 0 ;;
        *)
            usage_error "Invalid option '$1'" ;;
    esac
    shift
done

set -e

[[ -n ${bottlerocket_dir} ]] || usage_error 'required: --bottlerocket-dir'
[[ -n ${alpha_registry} ]] || usage_error 'required: --alpha-registry'
[[ -n ${alpha_version} ]] || usage_error 'required: --alpha-version'

variant="${variant:=aws-dev}"
sdk_name="${sdk_name:=bottlerocket-sdk}"
sdk_registry="${sdk_registry:=public.ecr.aws/bottlerocket}"
sdk_repo="${sdk_registry}/${sdk_name}"
alpha_name="${alpha_name:=twoliter-alpha-sdk}"
skip_clean="${skip_clean:=false}"

cd "${bottlerocket_dir}"

if [ -z "${sdk_version}" ]; then
  sdk_version=$(cat Twoliter.toml | grep bottlerocket-sdk -A1 | grep -Eoh '"[0-9.v]+"' | cut -d '"' -f2)
  echo "SDK Version '${sdk_version}' parsed from Twoliter.toml"
fi

for target_arch in x86_64 aarch64
do

  if [ "${skip_clean}" = "false" ]; then
    cargo make clean
  fi

  # We need the sbkeys scripts in a location that is not .dockerignored but is .gitignored
  rm -rf "${bottlerocket_dir}/build/sbkeys"
  mkdir -p "${bottlerocket_dir}/build/sbkeys"
  mkdir -p "${bottlerocket_dir}/.cargo/sbkeys"
  cp "${bottlerocket_dir}/sbkeys/generate-aws-sbkeys" "${bottlerocket_dir}/.cargo/sbkeys"
  cp "${bottlerocket_dir}/sbkeys/generate-local-sbkeys" "${bottlerocket_dir}/.cargo/sbkeys"

  # First we build aws-dev to make all of (or at least more of) the upstream packages available in
  # the event ${variant} does not include them.
  cargo make \
    -e "BUILDSYS_VARIANT=aws-dev" \
    -e "BUILDSYS_ARCH=${target_arch}" \
    build-variant

  cargo make \
    -e "BUILDSYS_VARIANT=aws-ecs-1" \
    -e "BUILDSYS_ARCH=${target_arch}" \
    build-variant

  cargo make \
    -e "BUILDSYS_VARIANT=${variant}" \
    -e "BUILDSYS_ARCH=${target_arch}" \
    build-variant
done

for host_arch in amd64 arm64
do
  sdk="${sdk_repo}:${sdk_version}"
  tag="${alpha_registry}/${alpha_name}:${alpha_version}-${host_arch}"
  echo "creating image ${tag}"

  docker build \
    --tag "${tag}" \
    --build-arg "SDK=${sdk}" \
    --build-arg "HOST_GOARCH=${host_arch}" \
    --file "${script_dir}/alpha-sdk.dockerfile" \
    "${bottlerocket_dir}"
  docker push "${tag}"
done

arm_host="${alpha_registry}/${alpha_name}:${alpha_version}-arm64"
amd_host="${alpha_registry}/${alpha_name}:${alpha_version}-amd64"
multiarch="${alpha_registry}/${alpha_name}:${alpha_version}"

echo "creating multiarch manifest ${multiarch}"
docker manifest rm "${multiarch}" || true
docker manifest create "${multiarch}" "${arm_host}" "${amd_host}"
echo "pushing multiarch manifest ${multiarch}"
docker manifest push "${multiarch}"
