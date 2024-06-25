# syntax=docker/dockerfile:1.4.3
# This Dockerfile has three sections which are used to build rpm.spec packages, to create
# kits, and to create Bottlerocket images, respectively. They are marked as Sections 1-3.
# buildsys uses Section 1 during build-package calls, Section 2 during build-kit calls,
# and Section 3 during build-variant calls.
#
# Several commands start with RUN --mount=target=/host, which mounts the docker build
# context (which in practice is the root of the Bottlerocket repository) as a read-only
# filesystem at /host.

ARG SDK
ARG ARCH
ARG GOARCH

FROM ${SDK} as sdk

############################################################################################
# Section 1: The following build stages are used to build rpm.spec packages

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# The experimental cache mount type doesn't expand arguments, so our choices are limited.
# We can either reuse the same cache for all builds, which triggers overlayfs errors if the
# builds run in parallel, or we can use a new cache for each build, which defeats the
# purpose. We work around the limitation by materializing a per-build stage that can be used
# as the source of the cache.
FROM scratch AS cache
ARG PACKAGE
ARG ARCH
ARG TOKEN
# We can't create directories via RUN in a scratch container, so take an existing one.
COPY --chown=1000:1000 --from=sdk /tmp /cache
# Ensure the ARG variables are used in the layer to prevent reuse by other builds.
COPY --chown=1000:1000 Twoliter.toml /cache/.${PACKAGE}.${ARCH}.${TOKEN}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Builds an RPM package from a spec file.
FROM sdk AS rpmbuild
ARG PACKAGE
ARG PACKAGE_DEPENDENCIES
ARG KIT_DEPENDENCIES
ARG EXTERNAL_KIT_DEPENDENCIES
ARG ARCH
ARG NOCACHE
ARG BUILD_ID
ARG BUILD_ID_TIMESTAMP
ENV BUILD_ID=${BUILD_ID}
ENV BUILD_ID_TIMESTAMP=${BUILD_ID_TIMESTAMP}
WORKDIR /home/builder

USER builder
ENV PACKAGE=${PACKAGE} ARCH=${ARCH}
COPY ./packages/${PACKAGE}/${PACKAGE}.spec .

# Copy over the target-specific macros, and put sources in the right place.
RUN \
   cp "/usr/lib/rpm/platform/${ARCH}-bottlerocket/macros" .rpmmacros \
   && cat ${PACKAGE}.spec >> rpmbuild/SPECS/${PACKAGE}.spec \
   && find . -maxdepth 1 -not -path '*/\.*' -type f -exec mv {} rpmbuild/SOURCES/ \; \
   && echo ${NOCACHE}

USER root
ARG BYPASS_SOCKET
RUN --mount=target=/host \
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    find /bypass/build/rpms/ -mindepth 1 -maxdepth 1 -name '*.rpm' -size +0c -print -exec \
      ln -snft ./rpmbuild/RPMS {} \+ && \
    for pkg in ${PACKAGE_DEPENDENCIES} ; do \
      [ -d "/bypass/build/rpms/${pkg}" ] || continue ; \
      find /bypass/build/rpms/${pkg}/ -mindepth 1 -maxdepth 1 -name '*.rpm' -size +0c -print -exec \
        ln -snft ./rpmbuild/RPMS {} \+ ; \
    done && \
    createrepo_c \
      -o ./rpmbuild/RPMS \
      -x '*-debuginfo-*.rpm' \
      -x '*-debugsource-*.rpm' \
      --no-database \
      ./rpmbuild/RPMS && \
    cp .rpmmacros /etc/rpm/macros && \
    declare -a KIT_REPOS && \
    for kit in ${KIT_DEPENDENCIES} ; do \
      KIT_REPOS+=("--repofrompath=${kit},/bypass/build/kits/${kit}/${ARCH}" --enablerepo "${kit}") ; \
    done && \
    echo "${KIT_REPOS[@]}" && \
    declare -a EXTERNAL_KIT_REPOS && \
    for kit in ${EXTERNAL_KIT_DEPENDENCIES} ; do \
      REPO_NAME="$(tr -s '/' '-' <<< "${kit}")" && \
      REPO_PATH="/bypass/build/external-kits/${kit}/${ARCH}" && \
      EXTERNAL_KIT_REPOS+=("--repofrompath=${REPO_NAME},${REPO_PATH}" --enablerepo "${REPO_NAME}"); \
    done && \
    echo "${EXTERNAL_KIT_REPOS[@]}" && \
    dnf -y \
      --disablerepo '*' \
      --repofrompath repo,./rpmbuild/RPMS \
      --enablerepo 'repo' \
      "${KIT_REPOS[@]}" \
      "${EXTERNAL_KIT_REPOS[@]}" \
      --nogpgcheck \
      --forcearch "${ARCH}" \
      builddep rpmbuild/SPECS/${PACKAGE}.spec && \
    find "/bypass/packages/${PACKAGE}" \
      -maxdepth 1 \
      -not -path '*/\.*' \
      -type f \
      -exec cp {} ./rpmbuild/SOURCES/ \; && \
    rm /bypass

# Ensure that the target binutils that `find-debuginfo.sh` uses are present in $PATH.
ENV PATH="/usr/${ARCH}-bottlerocket-linux-gnu/debuginfo/bin:${PATH}"

USER builder
RUN --mount=source=.cargo,target=/home/builder/.cargo \
    --mount=type=cache,target=/home/builder/.cache,from=cache,source=/cache \
    --mount=source=sources,target=/home/builder/rpmbuild/BUILD/sources \
    --mount=target=/host \
    # The dist tag is set as the `Release` field in Bottlerocket RPMs. Define it to be
    # in the form <timestamp of latest commit>.<latest commit short sha>.br1
    # Remove '-dirty' from the commit sha: '-' is an illegal character for the Release field
    # and '-dirty' may not be accurate to the state of the actual package being built.
    /host/build/tools/unplug \
      rpmbuild -bb --clean \
        --undefine _auto_set_build_flags \
        --define "_target_cpu ${ARCH}" \
        --define "dist .${BUILD_ID_TIMESTAMP}.${BUILD_ID//-dirty/}.br1" \
        rpmbuild/SPECS/${PACKAGE}.spec

# Copies RPM packages to the output directory that buildsys expects.
USER root
ARG BUILDER_UID
ARG OUTPUT_SOCKET
RUN --mount=target=/host \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    rm -rf /output/* && \
    cp /home/builder/rpmbuild/RPMS/*/*.rpm /output/ && \
    chown -R "${BUILDER_UID}:${BUILDER_UID}" /output/ && \
    rm -f /home/builder/rpmbuild/RPMS/*/*.rpm && \
    rm /output && \
    touch /tmp/.${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Finish up the package build stage.
FROM scratch AS package
ARG NOCACHE
COPY --from=rpmbuild /tmp/.${NOCACHE} /

############################################################################################
# Section 2: The following build stages are used to create a Bottlerocket kit once all of
# the rpm files have been created by repeatedly using Section 1. This process can occur more
# than once because packages can depend on kits and those kits depend on packages that must
# be built first.

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Builds a kit from RPM packages.
FROM sdk AS kitbuild
ARG KIT
ARG PACKAGE_DEPENDENCIES
ARG ARCH
ARG NOCACHE
ARG BUILD_ID
ARG VERSION_ID
ARG EXTERNAL_KIT_METADATA
ARG VENDOR
ARG LOCAL_KIT_DEPENDENCIES
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
ARG BUILDER_UID

WORKDIR /home/builder
USER root

RUN --mount=target=/host \
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    rm -rf /output/* && \
    /host/build/tools/rpm2kit \
        --packages-dir=/bypass/build/rpms \
        --arch="${ARCH}" \
        "${PACKAGE_DEPENDENCIES[@]/#/--package=}" \
        --output-dir=/output && \
    chown -R "${BUILDER_UID}:${BUILDER_UID}" /output/ && \
    rm /output && \
    rm /bypass && \
    touch /tmp/.${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Finish up the kit build stage.
FROM scratch AS kit
ARG NOCACHE
COPY --from=kitbuild /tmp/.${NOCACHE} /

############################################################################################
# Section 3: The following build stages are used to create a Bottlerocket image once all of
# the rpm files have been created by repeatedly using Sections 1 and 2.

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Generate the expected RPM macros and bconds.
FROM sdk as rpm-macros-and-bconds
ARG VARIANT
ARG VARIANT_PLATFORM
ARG VARIANT_RUNTIME
ARG VARIANT_FAMILY
ARG VARIANT_FLAVOR
ARG GRUB_SET_PRIVATE_VAR
ARG UEFI_SECURE_BOOT
ARG SYSTEMD_NETWORKD
ARG XFS_DATA_PARTITION
ARG FIPS

USER builder
WORKDIR /home/builder
RUN \
   export RPM_MACROS="generated.rpmmacros" \
   && export RPM_BCONDS="generated.bconds" \
   && echo "%_cross_variant ${VARIANT}" > "${RPM_MACROS}" \
   && echo "%_cross_variant_platform ${VARIANT_PLATFORM}" >> "${RPM_MACROS}" \
   && echo "%_cross_variant_runtime ${VARIANT_RUNTIME}" >> "${RPM_MACROS}" \
   && echo "%_cross_variant_family ${VARIANT_FAMILY}" >> "${RPM_MACROS}" \
   && echo "%_cross_variant_flavor ${VARIANT_FLAVOR:-none}" >> "${RPM_MACROS}" \
   && echo "%_topdir /home/builder/rpmbuild" >> "${RPM_MACROS}" \
   && echo "%bcond_without $(V=${VARIANT_PLATFORM,,}; echo ${V//-/_})_platform" > "${RPM_BCONDS}" \
   && echo "%bcond_without $(V=${VARIANT_RUNTIME,,}; echo ${V//-/_})_runtime" >> "${RPM_BCONDS}" \
   && echo "%bcond_without $(V=${VARIANT_FAMILY,,}; echo ${V//-/_})_family" >> "${RPM_BCONDS}" \
   && echo "%bcond_without $(V=${VARIANT_FLAVOR:-no}; V=${V,,}; echo ${V//-/_})_flavor" >> "${RPM_BCONDS}" \
   && echo -e -n "${GRUB_SET_PRIVATE_VAR:+%bcond_without grub_set_private_var\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${FIPS:+%bcond_without fips\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${UEFI_SECURE_BOOT:+%bcond_without uefi_secure_boot\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${SYSTEMD_NETWORKD:+%bcond_without systemd_networkd\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${XFS_DATA_PARTITION:+%bcond_without xfs_data_partition\n}" >> "${RPM_BCONDS}"

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Creates an RPM repository from packages created in Section 1 and kits from Section 2.
FROM rpm-macros-and-bconds AS repobuild
# The list of packages from the variant Cargo.toml package.metadata.build-variant.packages section.
ARG PACKAGES
# The complete list of non-kit packages required by way of pure package-to-package dependencies.
ARG PACKAGE_DEPENDENCIES
ARG KIT_DEPENDENCIES
ARG EXTERNAL_KIT_DEPENDENCIES
ARG ARCH
ARG NOCACHE

WORKDIR /home/builder
USER builder

# Build the metadata RPM for the variant.
RUN --mount=target=/host \
   cat "/usr/lib/rpm/platform/${ARCH}-bottlerocket/macros" generated.rpmmacros > .rpmmacros \
   && cat generated.bconds /host/build/tools/metadata.spec >> rpmbuild/SPECS/metadata.spec \
   && rpmbuild -ba --clean \
      --undefine _auto_set_build_flags \
      --define "_target_cpu ${ARCH}" \
      rpmbuild/SPECS/metadata.spec \
   && rpm -qp --provides rpmbuild/RPMS/${ARCH}/bottlerocket-metadata-*.${ARCH}.rpm \
   && echo ${NOCACHE}

WORKDIR /root
USER root
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
RUN --mount=target=/host \
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    rm -rf /output/* && \
    mkdir -p ./rpmbuild/RPMS && \
    find /bypass/build/rpms/ -mindepth 1 -maxdepth 1 -name "*.${ARCH}.rpm" -size +0c -print -exec \
      ln -snft ./rpmbuild/RPMS {} \+ && \
    for pkg in ${PACKAGE_DEPENDENCIES} ; do \
      [ -d "/bypass/build/rpms/${pkg}" ] || continue ; \
      find /bypass/build/rpms/${pkg}/ -mindepth 1 -maxdepth 1 -name "*.${ARCH}.rpm" -size +0c -print -exec \
        ln -snft ./rpmbuild/RPMS {} \+ ; \
    done && \
    ln -snf /home/builder/rpmbuild/RPMS/*/*.rpm ./rpmbuild/RPMS && \
    createrepo_c \
      -o ./rpmbuild/RPMS \
      -x '*-debuginfo-*.rpm' \
      -x '*-debugsource-*.rpm' \
      --no-database \
      ./rpmbuild/RPMS && \
    echo '%_dbpath %{_sharedstatedir}/rpm' >> /etc/rpm/macros && \
    declare -a KIT_REPOS && \
    for kit in ${KIT_DEPENDENCIES} ; do \
      KIT_REPOS+=("--repofrompath=${kit},/bypass/build/kits/${kit}/${ARCH}" --enablerepo "${kit}") ; \
    done && \
    declare -a EXTERNAL_KIT_REPOS && \
    for kit in ${EXTERNAL_KIT_DEPENDENCIES} ; do \
      REPO_NAME="$(tr -s '/' '-' <<< "${kit}")" && \
      REPO_PATH="/bypass/build/external-kits/${kit}/${ARCH}" && \
      EXTERNAL_KIT_REPOS+=("--repofrompath=${REPO_NAME},${REPO_PATH}" --enablerepo "${REPO_NAME}"); \
    done && \
    echo "${EXTERNAL_KIT_REPOS[@]}" && \
    dnf -y \
      --disablerepo '*' \
      --repofrompath repo,./rpmbuild/RPMS \
      --enablerepo 'repo' \
      "${KIT_REPOS[@]}" \
      "${EXTERNAL_KIT_REPOS[@]}" \
      --nogpgcheck \
      --downloadonly \
      --downloaddir . \
      --forcearch "${ARCH}" \
      install $(printf "bottlerocket-%s\n" metadata ${PACKAGES}) && \
    mkdir -p /local/rpms && \
    mv *.rpm /local/rpms && \
    createrepo_c /local/rpms && \
    rm /output && \
    rm /bypass && \
    echo ${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Builds a Bottlerocket image.
FROM repobuild as imgbuild
ARG ARCH
ARG VERSION_ID
ARG BUILD_ID
ARG NOCACHE
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
ARG BUILDER_UID
ARG VARIANT
ARG PRETTY_NAME
ARG IMAGE_NAME
ARG IMAGE_FORMAT
ARG OS_IMAGE_SIZE_GIB
ARG DATA_IMAGE_SIZE_GIB
ARG PARTITION_PLAN
ARG OS_IMAGE_PUBLISH_SIZE_GIB
ARG DATA_IMAGE_PUBLISH_SIZE_GIB
ARG KERNEL_PARAMETERS
ARG GRUB_SET_PRIVATE_VAR
ARG XFS_DATA_PARTITION
ARG UEFI_SECURE_BOOT
ENV VARIANT=${VARIANT} VERSION_ID=${VERSION_ID} BUILD_ID=${BUILD_ID} \
    PRETTY_NAME=${PRETTY_NAME} IMAGE_NAME=${IMAGE_NAME} \
    KERNEL_PARAMETERS=${KERNEL_PARAMETERS}
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
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    /host/build/tools/rpm2img \
      --package-dir=/local/rpms \
      --output-dir=/output \
      --external-kits-path="/bypass/build/external-kits" \
      --output-fmt="${IMAGE_FORMAT}" \
      --os-image-size-gib="${OS_IMAGE_SIZE_GIB}" \
      --data-image-size-gib="${DATA_IMAGE_SIZE_GIB}" \
      --os-image-publish-size-gib="${OS_IMAGE_PUBLISH_SIZE_GIB}" \
      --data-image-publish-size-gib="${DATA_IMAGE_PUBLISH_SIZE_GIB}" \
      --partition-plan="${PARTITION_PLAN}" \
      --ovf-template="/bypass/variants/${VARIANT}/template.ovf" \
      ${XFS_DATA_PARTITION:+--xfs-data-partition=yes} \
      ${GRUB_SET_PRIVATE_VAR:+--with-grub-set-private-var=yes} \
      ${UEFI_SECURE_BOOT:+--with-uefi-secure-boot=yes} && \
    rm -rf /local/rpms && \
    chown -R "${BUILDER_UID}:${BUILDER_UID}" /output/ && \
    rm /output && \
    rm /bypass && \
    touch /tmp/.${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Creates an archive of the datastore migrations.
FROM repobuild as migrationbuild
ARG ARCH
ARG VERSION_ID
ARG BUILD_ID
ARG NOCACHE
ARG VARIANT
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
ARG BUILDER_UID
ENV VARIANT=${VARIANT} VERSION_ID=${VERSION_ID} BUILD_ID=${BUILD_ID}
WORKDIR /root

USER root
RUN --mount=target=/host \
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    mkdir -p /local/migrations && \
    find /bypass/build/rpms/ -maxdepth 2 -type f \
        -name "bottlerocket-migrations-*.rpm" \
        -not -iname '*debuginfo*' \
        -exec cp '{}' '/local/migrations/' ';' && \
    /host/build/tools/rpm2migrations \
        --package-dir=/local/migrations \
        --output-dir=/output && \
    chown -R "${BUILDER_UID}:${BUILDER_UID}" /output && \
    rm -rf /local/migrations && \
    rm /output && \
    rm /bypass && \
    touch /tmp/.${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Creates an archive of kernel development sources and toolchain.
FROM repobuild as kmodkitbuild
# The list of packages from the variant Cargo.toml package.metadata.build-variant.packages section.
ARG PACKAGES
ARG ARCH
ARG VERSION_ID
ARG BUILD_ID
ARG NOCACHE
ARG VARIANT
ENV VARIANT=${VARIANT} VERSION_ID=${VERSION_ID} BUILD_ID=${BUILD_ID}
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
ARG BUILDER_UID

USER root

WORKDIR /tmp
RUN --mount=target=/host \
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    mkdir -p /local/archives && \
    KERNEL="$(printf "%s\n" ${PACKAGES} | awk '/^kernel-/{print $1}')" && \
    find /bypass/build/ -type f \
        -name "bottlerocket-${KERNEL}-archive-*.${ARCH}.rpm" \
        -exec cp '{}' '/local/archives/' ';' && \
    /host/build/tools/rpm2kmodkit \
        --archive-dir=/local/archives \
        --toolchain-dir=/toolchain \
        --output-dir=/output && \
    chown -R "${BUILDER_UID}:${BUILDER_UID}" /output/ && \
    rm -rf /local/archives && \
    rm /output && \
    rm /bypass && \
    touch /tmp/.${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Finish up the variant build stage.
FROM scratch AS variant
ARG NOCACHE
COPY --from=imgbuild /tmp/.${NOCACHE} /output/
COPY --from=migrationbuild /tmp/.${NOCACHE} /output/
COPY --from=kmodkitbuild /tmp/.${NOCACHE} /output/

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Repack an existing image.
FROM sdk as imgrepack
ARG ARCH
ARG VERSION_ID
ARG BUILD_ID
ARG NOCACHE
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
ARG BUILDER_UID
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
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    rm -rf /output/* && \
    /host/build/tools/img2img \
      --input-dir="/bypass/build/images/${ARCH}-${VARIANT}/${VERSION_ID}-${BUILD_ID}" \
      --output-dir=/output \
      --output-fmt="${IMAGE_FORMAT}" \
      --os-image-size-gib="${OS_IMAGE_SIZE_GIB}" \
      --data-image-size-gib="${DATA_IMAGE_SIZE_GIB}" \
      --os-image-publish-size-gib="${OS_IMAGE_PUBLISH_SIZE_GIB}" \
      --data-image-publish-size-gib="${DATA_IMAGE_PUBLISH_SIZE_GIB}" \
      --partition-plan="${PARTITION_PLAN}" \
      --ovf-template="/bypass/variants/${VARIANT}/template.ovf" \
      ${UEFI_SECURE_BOOT:+--with-uefi-secure-boot=yes} && \
    chown -R "${BUILDER_UID}:${BUILDER_UID}" /output/ && \
    rm /output && \
    rm /bypass && \
    touch /tmp/.${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Finish up the image repack stage.
FROM scratch AS repack
ARG NOCACHE
COPY --from=imgrepack /tmp/.${NOCACHE} /output/
