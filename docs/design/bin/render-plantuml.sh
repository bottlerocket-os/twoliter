#!/usr/bin/env bash
set -ea

script_dir=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
ignore_dir="${script_dir}/../../.ignore"
mkdir -p "${ignore_dir}"
ignore_dir=$( cd "${ignore_dir}" &> /dev/null && pwd )
CARGO_HOME="${ignore_dir}/.cargo"
planturl="${CARGO_HOME}/bin/planturl"
input="${1}"
output="${2}"
image="plantuml/plantuml-server:tomcat"


if [[ -z "${input}" ]] | [[ -z "${output}" ]]; then
  echo "usage: render-plantuml ./input.file ./output.file"
  exit 1
fi


cargo install --features="build-binary" planturl
data="$("${planturl}" --source "${input}" --type "svg")"
url="http://localhost:8080/svg/${data}"

docker rm --force render_plantuml &> /dev/null || true
docker run --rm -d --name render_plantuml -p 8080:8080 "${image}"

# wait an arbitrary amount of time for the server to be ready
sleep 1

curl "${url}" > "${output}"

docker stop render_plantuml &> /dev/null
docker rm --force render_plantuml &> /dev/null || true
