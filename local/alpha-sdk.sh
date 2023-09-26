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
Usage: $0 --bottlerocket-dir DIR [--variant VARIANT] [--arch ARCH] --sdk-version SDK_VERSION [--sdk-name SDK_NAME] [ --sdk-registry SDK_REGISTRY ] [-h]

    --bottlerocket-dir            The directory of the Bottlerocket checkout that we will build.
    --variant                     The Bottlerocket variant that we will build.
    --arch                        The target architecture that we will build Bottlerocket for.
                                  Defaults to this host's architecture.
    --sdk-version                 The version of the Bottlerocket SDK to use when building packages
                                  and to use as the base for the Twoliter alpha-sdk. For example if
                                  the SDK version is v0.50.0 then --sdk-version should be the same
                                  (i.e. with the v prefix).
    --sdk-name                    The name prefix of the SDK. For example, in this following string
                                  'bottlerocket' is the SDK name:
                                  public.ecr.aws/bottlerocket/bottlerocket-sdk-x86_64:v0.50.0
                                  Note that the suffix '-sdk' is assumed and added to the name,
                                  which is just 'bottlerocket'
    --sdk-registry                The namespace or Docker registry where the SDK is found. For
                                  example, in the following string 'public.ecr.aws' is the
                                  registry:
                                  public.ecr.aws/bottlerocket/bottlerocket-sdk-x86_64:v0.50.0
    --alpha-sdk-tag               The tag to give the Docker image that this script produces.
    -h, --help                    show this help text

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
        --arch)
            shift; arch=$1 ;;
        --sdk-version)
            shift; sdk_version=$1 ;;
        --sdk-name)
            shift; sdk_name=$1 ;;
        --sdk-registry)
            shift; sdk_registry=$1 ;;
        --tag)
            shift; tag=$1 ;;
        -h|--help)
            usage; exit 0 ;;
        *)
            usage_error "Invalid option '$1'" ;;
    esac
    shift
done

set -e

[[ -n ${bottlerocket_dir} ]] || usage_error 'required: --bottlerocket-dir'
[[ -n ${bottlerocket_dir} ]] || usage_error 'required: --sdk-version'

variant="${variant:=aws-dev}"
arch="${arch:=$(uname -m)}"
sdk_name="${sdk_name:=bottlerocket}"
sdk_registry="${sdk_registry:=public.ecr.aws/bottlerocket}"
tag="${tag:=twoliter.alpha/bottlerocket-sdk}"

cd "${bottlerocket_dir}"

cargo make \
    -e "BUILDSYS_VARIANT=${variant}" \
    -e "BUILDSYS_ARCH=${arch}" \
    -e "BUILDSYS_SDK_NAME=${sdk_name}" \
    -e "BUILDSYS_SDK_VERSION=${sdk_version}" \
    -e "BUILDSYS_SDK_REGISTRY=${sdk_registry}" \
    build-variant

sdk="${sdk_registry}/${sdk_name}-sdk-${arch}:${sdk_version}"

docker build \
  --tag "${tag}" \
  --build-arg "SDK=${sdk}" \
  --file "${script_dir}/alpha-sdk.dockerfile" \
  "${bottlerocket_dir}"
