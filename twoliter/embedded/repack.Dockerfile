# syntax=docker/dockerfile:1.4.3
#
# Several commands start with RUN --mount=target=/host, which mounts the docker build
# context (which in practice is the root of the Bottlerocket repository) as a read-only
# filesystem at /host.
ARG SDK

FROM ${SDK} as imgrepack

ARG ARCH
ARG VERSION_ID
ARG BUILD_ID
ARG NOCACHE
ARG VARIANT
ARG IMAGE_NAME
ARG IMAGE_FORMAT
ARG OS_IMAGE_SIZE_GIB
ARG DATA_IMAGE_SIZE_GIB
ARG PARTITION_PLAN
ARG OS_IMAGE_PUBLISH_SIZE_GIB
ARG DATA_IMAGE_PUBLISH_SIZE_GIB
ARG UEFI_SECURE_BOOT
ENV VARIANT=${VARIANT} VERSION_ID=${VERSION_ID} BUILD_ID=${BUILD_ID}
WORKDIR /root

USER root
RUN --mount=target=/host \
    --mount=type=secret,id=ca-bundle.crt,target=/root/certs/ca-bundle.crt \
    --mount=type=secret,id=root.json,target=/root/roles/root.json \
    --mount=type=secret,id=PK.crt,target=/root/sbkeys/PK.crt \
    --mount=type=secret,id=KEK.crt,target=/root/sbkeys/KEK.crt \
    --mount=type=secret,id=db.crt,target=/root/sbkeys/db.crt \
    --mount=type=secret,id=vendor.crt,target=/root/sbkeys/vendor.crt \
    --mount=type=secret,id=shim-sign.key,target=/root/sbkeys/shim-sign.key \
    --mount=type=secret,id=shim-sign.crt,target=/root/sbkeys/shim-sign.crt \
    --mount=type=secret,id=code-sign.key,target=/root/sbkeys/code-sign.key \
    --mount=type=secret,id=code-sign.crt,target=/root/sbkeys/code-sign.crt \
    --mount=type=secret,id=config-sign.key,target=/root/sbkeys/config-sign.key \
    --mount=type=secret,id=kms-sign.json,target=/root/.config/aws-kms-pkcs11/config.json \
    --mount=type=secret,id=aws-access-key-id.env,target=/root/.aws/aws-access-key-id.env \
    --mount=type=secret,id=aws-secret-access-key.env,target=/root/.aws/aws-secret-access-key.env \
    --mount=type=secret,id=aws-session-token.env,target=/root/.aws/aws-session-token.env \
    /host/build/tools/img2img \
      --input-dir="/host/build/images/${ARCH}-${VARIANT}/${VERSION_ID}-${BUILD_ID}" \
      --output-dir=/local/output \
      --output-fmt="${IMAGE_FORMAT}" \
      --os-image-size-gib="${OS_IMAGE_SIZE_GIB}" \
      --data-image-size-gib="${DATA_IMAGE_SIZE_GIB}" \
      --os-image-publish-size-gib="${OS_IMAGE_PUBLISH_SIZE_GIB}" \
      --data-image-publish-size-gib="${DATA_IMAGE_PUBLISH_SIZE_GIB}" \
      --partition-plan="${PARTITION_PLAN}" \
      --ovf-template="/host/variants/${VARIANT}/template.ovf" \
      ${UEFI_SECURE_BOOT:+--with-uefi-secure-boot=yes} \
    && echo ${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Copies the repackaged artifacts to their expected location so that buildsys can find them
# and copy them out.
FROM scratch AS repack
COPY --from=imgrepack /local/output/. /output/
