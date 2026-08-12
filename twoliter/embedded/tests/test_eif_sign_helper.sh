#!/usr/bin/env bash
#
# Test `discover_eif_signer_args` across the four EIF-signing-profile shapes
# that Infra.toml's [eif] section (via buildsys and build.Dockerfile) can
# produce inside the build container:
#
#   1. Local:      signing.crt + signing.key
#   2. KMS:        signing.crt + kms-key-id (a non-empty file whose contents
#                                            are the KMS key id/ARN/alias)
#   3. Cert-only:  signing.crt but neither backend (error path — indicates
#                                                   broken build plumbing)
#   4. None:       no signing.crt at all (no [eif] section in Infra.toml, or
#                                         no Infra.toml passed to buildsys)
#
# Run from the repo root:
#   bash twoliter/embedded/tests/test_eif_sign_helper.sh
#
# Complements `test_rpm2eif_args.sh` (flag validation) and pins the exact
# tuple that rpm2eif, eif2eif, and img2img all rely on.

set -eu -o pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../eif-sign-helper"

if [[ ! -f "${HELPER}" ]]; then
  echo "test_eif_sign_helper: helper not found at ${HELPER}" >&2
  exit 1
fi

pass_count=0
fail_count=0

pass() { pass_count=$((pass_count + 1)); echo "  ok: $1"; }
fail() { fail_count=$((fail_count + 1)); echo "  FAIL: $1" >&2; }

# Run `discover_eif_signer_args` in a fresh subshell against a synthesized
# signing directory and print the resulting argv (one entry per line).
# Args:
#   $1  scenario name (unused, for readability)
#   $2  directory to expose as EIF_SIGNING_DIR (the fixture)
#   $3  optional: directory to expose as EIF_AWS_DIR (AWS credential env
#       file fixture; only consulted on the KMS backend). Defaults to an
#       empty temp dir so the credential-export block is a no-op.
run_helper() {
  local dir="$2"
  local aws_dir="${3:-${tmp}/no-aws-creds}"
  mkdir -p "${aws_dir}"
  (
    # Start from a clean AWS env so the "export AWS credentials on the
    # KMS path" invariant can be checked precisely — otherwise the
    # invoker's ambient AWS_ACCESS_KEY_ID (a common shape on CI runners
    # and dev laptops) would leak through and look like a successful
    # export even when the helper never touched the value.
    unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN
    export EIF_SIGNING_DIR="${dir}"
    export EIF_AWS_DIR="${aws_dir}"
    # shellcheck disable=SC1090
    . "${HELPER}"
    declare -a out
    if discover_eif_signer_args out; then
      printf '%s\n' "${out[@]}"
      echo "__RC=0"
      # Dump the AWS_* env so tests can assert on credential export.
      echo "__AWS_ACCESS_KEY_ID=${AWS_ACCESS_KEY_ID-<unset>}"
      echo "__AWS_SECRET_ACCESS_KEY=${AWS_SECRET_ACCESS_KEY-<unset>}"
      echo "__AWS_SESSION_TOKEN=${AWS_SESSION_TOKEN-<unset>}"
    else
      printf '%s\n' "${out[@]-}"
      echo "__RC=1"
    fi
  ) 2>&1
}

# Extract the `__RC=` line's value from run_helper's output.
extract_rc() {
  local out="$1"
  local rc
  rc=$(printf '%s\n' "${out}" | grep -E '^__RC=' | tail -1 | cut -d= -f2)
  echo "${rc:-?}"
}

tmp=$(mktemp -d)
trap 'rm -rf "${tmp}"' EXIT

# --------------------------------------------------------------------------
# Scenario 1: local backend (signing.crt + signing.key both present)
# --------------------------------------------------------------------------
dir1="${tmp}/local"
mkdir -p "${dir1}"
echo "cert-a" >"${dir1}/signing.crt"
echo "key-a"  >"${dir1}/signing.key"

out=$(run_helper "local" "${dir1}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "0" ]] \
   && grep -q "^--signing-cert$"        <<<"${out}" \
   && grep -q "^${dir1}/signing.crt$"   <<<"${out}" \
   && grep -q "^--signing-key$"         <<<"${out}" \
   && grep -q "^${dir1}/signing.key$"   <<<"${out}" \
   && ! grep -q "^--kms-key-id$"        <<<"${out}"; then
  pass "local backend → --signing-cert + --signing-key"
else
  fail "local backend args wrong. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 2: KMS backend (signing.crt + kms-key-id file)
# --------------------------------------------------------------------------
dir2="${tmp}/kms"
mkdir -p "${dir2}"
echo "cert-b" >"${dir2}/signing.crt"
# The buildkit `env=` secret source lands the env var's value in the mount
# file. Simulate that: a bare id, no key file on disk.
echo -n "alias/my-eif-key" >"${dir2}/kms-key-id"

out=$(run_helper "kms" "${dir2}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "0" ]] \
   && grep -q "^--kms-key-id$"          <<<"${out}" \
   && grep -q "^alias/my-eif-key$"      <<<"${out}" \
   && grep -q "^--signing-cert$"        <<<"${out}" \
   && grep -q "^${dir2}/signing.crt$"   <<<"${out}" \
   && ! grep -q "^--signing-key$"       <<<"${out}"; then
  pass "KMS backend → --kms-key-id + --signing-cert"
else
  fail "KMS backend args wrong. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 2b: KMS backend with a full ARN, incidental trailing newline
# (defensive: some tools terminate env-source values with a newline). The
# helper strips whitespace before passing the id to eif-builder.
# --------------------------------------------------------------------------
dir2b="${tmp}/kms-arn"
mkdir -p "${dir2b}"
echo "cert-c" >"${dir2b}/signing.crt"
printf 'arn:aws:kms:us-east-1:0:key/00000000\n' >"${dir2b}/kms-key-id"

out=$(run_helper "kms-arn" "${dir2b}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "0" ]] \
   && grep -q "^--kms-key-id$"                                <<<"${out}" \
   && grep -q "^arn:aws:kms:us-east-1:0:key/00000000$"        <<<"${out}"; then
  pass "KMS backend accepts a full ARN with trailing newline"
else
  fail "KMS ARN wrong. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 2c: KMS backend with an explicit region (Infra.toml carries
# `[eif].signing_key.kms.region`; buildsys mounts it as a sidecar
# `kms-region` file). The helper appends `--region <r>` to the argv.
# --------------------------------------------------------------------------
dir2c="${tmp}/kms-region"
mkdir -p "${dir2c}"
echo "cert-region" >"${dir2c}/signing.crt"
echo -n "alias/my-eif-key" >"${dir2c}/kms-key-id"
echo -n "us-west-2"        >"${dir2c}/kms-region"

out=$(run_helper "kms-region" "${dir2c}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "0" ]] \
   && grep -q "^--kms-key-id$"    <<<"${out}" \
   && grep -q "^--region$"        <<<"${out}" \
   && grep -q "^us-west-2$"       <<<"${out}"; then
  pass "KMS backend with region → --region us-west-2"
else
  fail "KMS region wrong. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 2d: KMS backend with an empty kms-region file. This is the
# shape buildkit produces when the `env=EIF_KMS_REGION` source is unset
# (Infra.toml omits the field). The helper must skip `--region` and let
# the SDK's ambient chain handle it — no extra `--region` argv entry, no
# spurious `--region <empty>`.
# --------------------------------------------------------------------------
dir2d="${tmp}/kms-empty-region"
mkdir -p "${dir2d}"
echo "cert-empty-region" >"${dir2d}/signing.crt"
echo -n "alias/x"        >"${dir2d}/kms-key-id"
: >"${dir2d}/kms-region"

out=$(run_helper "kms-empty-region" "${dir2d}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "0" ]] \
   && grep -q "^--kms-key-id$"    <<<"${out}" \
   && ! grep -q "^--region$"      <<<"${out}"; then
  pass "KMS backend with empty region file → no --region flag"
else
  fail "empty kms-region should not emit --region. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 2e: KMS backend exports AWS_* env from the mounted credential
# `.env` files. The `.env` file contents land verbatim in the mount; the
# helper must read them and export them into the caller's environment so
# `aws-config::from_env()` inside `eif-builder` can pick them up. Without
# this export, every KMS-signed build fails with a credentials-not-found
# error even when the credentials are mounted.
# --------------------------------------------------------------------------
dir2e="${tmp}/kms-with-creds"
mkdir -p "${dir2e}"
echo "cert-creds"       >"${dir2e}/signing.crt"
echo -n "alias/y"       >"${dir2e}/kms-key-id"
aws_e="${tmp}/aws-creds-2e"
mkdir -p "${aws_e}"
echo -n "AKIAEXAMPLE"                          >"${aws_e}/aws-access-key-id.env"
echo -n "secret-example-1234"                  >"${aws_e}/aws-secret-access-key.env"
echo -n "session-token-example"                >"${aws_e}/aws-session-token.env"

out=$(run_helper "kms-with-creds" "${dir2e}" "${aws_e}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "0" ]] \
   && grep -q '^__AWS_ACCESS_KEY_ID=AKIAEXAMPLE$'          <<<"${out}" \
   && grep -q '^__AWS_SECRET_ACCESS_KEY=secret-example-1234$' <<<"${out}" \
   && grep -q '^__AWS_SESSION_TOKEN=session-token-example$'   <<<"${out}"; then
  pass "KMS backend exports AWS credential env from mounted files"
else
  fail "KMS creds not exported. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 2f: local backend must NOT touch the AWS env even when
# credential files happen to be mounted (they are unrelated to the local
# signing path; touching them would surprise anyone debugging a local
# build).
# --------------------------------------------------------------------------
dir2f="${tmp}/local-with-aws-mount"
mkdir -p "${dir2f}"
echo "cert-l"  >"${dir2f}/signing.crt"
echo "key-l"   >"${dir2f}/signing.key"
aws_f="${tmp}/aws-creds-2f"
mkdir -p "${aws_f}"
echo -n "SHOULD_NOT_BE_EXPORTED" >"${aws_f}/aws-access-key-id.env"

out=$(run_helper "local-with-aws-mount" "${dir2f}" "${aws_f}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "0" ]] \
   && grep -q "^--signing-key$"   <<<"${out}" \
   && ! grep -q "^__AWS_ACCESS_KEY_ID=SHOULD_NOT_BE_EXPORTED$" <<<"${out}"; then
  pass "local backend leaves AWS env untouched"
else
  fail "local backend must not export AWS creds. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 3: cert-only (error path — no backend at all)
# --------------------------------------------------------------------------
dir3="${tmp}/cert-only"
mkdir -p "${dir3}"
echo "cert-d" >"${dir3}/signing.crt"

out=$(run_helper "cert-only" "${dir3}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "1" ]] && grep -q "refusing to build an unsigned EIF" <<<"${out}"; then
  pass "cert-only fails hard (no backend files)"
else
  fail "cert-only should have failed. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 3b: cert-only with an empty kms-key-id file (buildkit mounts a
# zero-byte file when its env source is unset). Should behave the same as
# scenario 3 — `-s` requires size >0.
# --------------------------------------------------------------------------
dir3b="${tmp}/cert-only-empty-kms"
mkdir -p "${dir3b}"
echo "cert-e" >"${dir3b}/signing.crt"
: >"${dir3b}/kms-key-id"

out=$(run_helper "cert-only-empty-kms" "${dir3b}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "1" ]] && grep -q "refusing to build an unsigned EIF" <<<"${out}"; then
  pass "cert + empty kms-key-id file fails hard"
else
  fail "cert + empty kms-key-id should fail. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 3c: kms-key-id file that is only whitespace (post-strip, empty).
# We surface a distinct diagnostic for this shape so an operator can tell it
# apart from "backend file absent entirely".
# --------------------------------------------------------------------------
dir3c="${tmp}/cert-kms-whitespace"
mkdir -p "${dir3c}"
echo "cert-f" >"${dir3c}/signing.crt"
printf '   \n\t\n' >"${dir3c}/kms-key-id"

out=$(run_helper "cert-kms-whitespace" "${dir3c}")
rc=$(extract_rc "${out}")
if [[ "${rc}" == "1" ]] && grep -q "empty after stripping whitespace" <<<"${out}"; then
  pass "cert + whitespace-only kms-key-id fails with a specific diagnostic"
else
  fail "whitespace-only kms-key-id should fail. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 4: no profile at all (no signing.crt on disk)
# --------------------------------------------------------------------------
dir4="${tmp}/nothing"
mkdir -p "${dir4}"

out=$(run_helper "no-profile" "${dir4}")
rc=$(extract_rc "${out}")
non_meta=$(grep -Ev '^__RC=|^__AWS_|^eif-sign-helper' <<<"${out}" | grep -vE '^$' || true)
if [[ "${rc}" == "0" ]] && [[ -z "${non_meta}" ]]; then
  pass "no profile → empty args, rc 0"
else
  fail "no profile args wrong. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
# Scenario 5: empty signing.crt (buildkit-created zero-byte file when the
# secret source is missing). Treated the same as "no cert" — `[[ -s ]]`
# requires size >0.
# --------------------------------------------------------------------------
dir5="${tmp}/empty-cert"
mkdir -p "${dir5}"
: >"${dir5}/signing.crt"
# A stray key file present — this must NOT flip us into the "local backend"
# branch, because signing.crt is empty (`-s` returns false).
echo "key-only" >"${dir5}/signing.key"

out=$(run_helper "empty-cert" "${dir5}")
rc=$(extract_rc "${out}")
non_meta=$(grep -Ev '^__RC=|^__AWS_|^eif-sign-helper' <<<"${out}" | grep -vE '^$' || true)
if [[ "${rc}" == "0" ]] && [[ -z "${non_meta}" ]]; then
  pass "empty signing.crt treated as no profile (unsigned)"
else
  fail "empty signing.crt should produce unsigned. rc=${rc} out=${out}"
fi

# --------------------------------------------------------------------------
echo
echo "test_eif_sign_helper: ${pass_count} passed, ${fail_count} failed"
[[ "${fail_count}" -eq 0 ]]
