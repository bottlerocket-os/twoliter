# Twoliter requires minimum Docker version 23.0.0, which ships with a bundled syntax image 1.4.3
# We refrain from an explicit `syntax` directive as this can lead to unwanted network requests at
# build time.
# See https://hub.docker.com/r/docker/dockerfile for more information

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

FROM ${SDK} AS sdk

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

RUN --mount=target=/host \
    if [ -d "/host/advisories/" ]; then \
        /host/build/tools/advisory-checker \
            --packages-dir /host/packages/ \
            --advisories-dir /host/advisories/; \
    else \
        echo "Advisories directory not found, skipping check."; \
    fi

RUN --mount=target=/host \
    cp /host/build/tools/builder-group.spec rpmbuild/SPECS/builder-group.spec \
    && rpmbuild -bb --clean \
         --undefine _auto_set_build_flags \
         --define "_target_cpu ${ARCH}" \
         rpmbuild/SPECS/builder-group.spec

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
    cp .rpmmacros /root/.rpmmacros && \
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

USER builder
    # We only mount the config.toml and the vendor directory instead of the whole .cargo directory
    # because the .git and .registry directories are specific to the version of cargo and can cause
    # incapatibility between the host cargo and the cargo in the sdk
RUN --mount=source=.cargo/twoliter_cargo_config.toml,target=/home/builder/rpmbuild/.cargo/config.toml \
    --mount=source=.cargo/vendor,target=/home/builder/rpmbuild/.cargo/vendor \
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
    find /home/builder/rpmbuild/RPMS -type f -name 'bottlerocket-builder-group-*.rpm' -delete && \
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
FROM sdk AS rpm-macros-and-bconds
ARG VARIANT
ARG VARIANT_PLATFORM
ARG VARIANT_RUNTIME
ARG VARIANT_FAMILY
ARG VARIANT_FLAVOR
ARG UEFI_SECURE_BOOT
ARG XFS_DATA_PARTITION
ARG EROFS_ROOT_PARTITION
ARG IN_PLACE_UPDATES
ARG HOST_CONTAINERS
ARG FIPS
ARG EXTERNAL_KMOD_DEVELOPMENT
ARG ENCRYPTED_STORAGE
ARG STANDALONE_IMAGE

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
   && echo -e -n "${FIPS:+%bcond_without fips\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${UEFI_SECURE_BOOT:+%bcond_without uefi_secure_boot\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${XFS_DATA_PARTITION:+%bcond_without xfs_data_partition\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${EROFS_ROOT_PARTITION:+%bcond_without erofs_root_partition\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${IN_PLACE_UPDATES:+%bcond_without in_place_updates\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${HOST_CONTAINERS:+%bcond_without host_containers\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${EXTERNAL_KMOD_DEVELOPMENT:+%bcond_without external_kmod_development\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${ENCRYPTED_STORAGE:+%bcond_without encrypted_storage\n}" >> "${RPM_BCONDS}" \
   && echo -e -n "${STANDALONE_IMAGE:+%bcond_without standalone_image\n}" >> "${RPM_BCONDS}"

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
    mkdir -p /local/rpms /local/sbom-rpms && \
    download_dir_opt="$(dnf install --help | grep -Fwq downloaddir && echo "--downloaddir /local/rpms" ||:)" && \
    dnf -y \
      --disablerepo '*' \
      --repofrompath repo,./rpmbuild/RPMS \
      --enablerepo 'repo' \
      "${KIT_REPOS[@]}" \
      "${EXTERNAL_KIT_REPOS[@]}" \
      --nogpgcheck \
      --forcearch "${ARCH}" \
      --setopt=cachedir=/local/rpms \
      install \
        --downloadonly \
	${download_dir_opt} \
        $(printf "bottlerocket-%s\n" metadata metadata-sbom ${PACKAGES}) && \
    find /local/rpms -mindepth 2 -type f -name '*-sbom-*.rpm' -exec mv -t /local/sbom-rpms {} + && \
    find /local/rpms -mindepth 2 -type f -name '*.rpm' -exec mv -t /local/rpms {} + && \
    find /local/rpms -mindepth 1 ! -name '*.rpm' -delete && \
    rm /output && \
    rm /bypass && \
    echo ${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Builds a Bottlerocket image.
FROM repobuild AS imgbuild
ARG ARCH
ARG VERSION_ID
ARG BUILD_ID
ARG NOCACHE
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
ARG BUILDER_UID
ARG VARIANT
ARG VARIANT_NAME=${VARIANT}
ARG VARIANT_PLATFORM
ARG PRETTY_NAME
ARG IMAGE_NAME
ARG IMAGE_FORMAT
ARG OS_IMAGE_SIZE_GIB
ARG DATA_IMAGE_SIZE_GIB
ARG PARTITION_PLAN
ARG OS_IMAGE_PUBLISH_SIZE_GIB
ARG DATA_IMAGE_PUBLISH_SIZE_GIB
ARG KERNEL_PARAMETERS
ARG XFS_DATA_PARTITION
ARG EROFS_ROOT_PARTITION
ARG UEFI_SECURE_BOOT
ARG IN_PLACE_UPDATES
ARG ENCRYPTED_STORAGE
ARG STANDALONE_IMAGE
ARG PROJECT_VENDOR
ARG TWOLITER_VERSION
ARG GUEST_IMAGES
# BUILD_ID_TIMESTAMP: git commit Unix seconds. Set by buildsys (see
# `Builder::build_arg` in tools/buildsys/src/builder.rs). Consumed by
# `rpm2eif` to derive the reproducible RFC 3339 `BuildTime` written into
# the EIF METADATA section. Also baked into the RPM `dist` release string
# in the earlier `rpmbuild` stage — we forward it here separately because
# `imgbuild` inherits from `repobuild`, not from `rpmbuild`, so the
# rpmbuild ENV does not reach this stage.
ARG BUILD_ID_TIMESTAMP
ENV VARIANT=${VARIANT_NAME} VARIANT_PLATFORM=${VARIANT_PLATFORM} \
    VERSION_ID=${VERSION_ID} BUILD_ID=${BUILD_ID} \
    BUILD_ID_TIMESTAMP=${BUILD_ID_TIMESTAMP} \
    PRETTY_NAME=${PRETTY_NAME} IMAGE_NAME=${IMAGE_NAME} \
    KERNEL_PARAMETERS=${KERNEL_PARAMETERS} \
    TWOLITER_VERSION=${TWOLITER_VERSION}
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
    --mount=type=secret,id=efi-vars.json,target=/root/sbkeys/efi-vars.json \
    --mount=type=secret,id=kms-sign.json,target=/root/.config/aws-kms-pkcs11/config.json \
    --mount=type=secret,id=aws-access-key-id.env,target=/root/.aws/aws-access-key-id.env \
    --mount=type=secret,id=aws-secret-access-key.env,target=/root/.aws/aws-secret-access-key.env \
    --mount=type=secret,id=aws-session-token.env,target=/root/.aws/aws-session-token.env \
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    if [ "${IMAGE_FORMAT}" = "eif" ]; then \
      # EIF variants produce a self-contained sidecar EIF plus a GPT disk \
      # image and bare kernel. `rpm2eif` accepts the same feature flags as \
      # `rpm2img` and validates each one -- most are hard-rejected for EIF \
      # (secure-boot, IPU, encrypted-storage, xfs-data, non-erofs root) so \
      # that misconfiguration surfaces at build time rather than being \
      # silently ignored. `--os-image-size-gib` is honored: when set, the \
      # GPT disk image is padded to that exact size (useful for downstream \
      # host-embedding steps). \
      /host/build/tools/rpm2eif \
        --package-dir=/local/rpms \
        --sbom-package-dir=/local/sbom-rpms \
        --output-dir=/output \
        --external-kits-path="/bypass/build/external-kits" \
        --os-image-size-gib="${OS_IMAGE_SIZE_GIB}" \
        ${XFS_DATA_PARTITION:+--with-xfs-data-partition=yes} \
        ${EROFS_ROOT_PARTITION:+--with-erofs-root-partition=yes} \
        ${UEFI_SECURE_BOOT:+--with-uefi-secure-boot=yes} \
        ${IN_PLACE_UPDATES:+--with-in-place-updates=yes} \
        ${ENCRYPTED_STORAGE:+--with-encrypted-storage=yes} \
        --with-standalone-image="${STANDALONE_IMAGE:-no}" ; \
    else \
      /host/build/tools/rpm2img \
        --package-dir=/local/rpms \
        --sbom-package-dir=/local/sbom-rpms \
        --output-dir=/output \
        --external-kits-path="/bypass/build/external-kits" \
        --output-fmt="${IMAGE_FORMAT}" \
        --os-image-size-gib="${OS_IMAGE_SIZE_GIB}" \
        --data-image-size-gib="${DATA_IMAGE_SIZE_GIB}" \
        --os-image-publish-size-gib="${OS_IMAGE_PUBLISH_SIZE_GIB}" \
        --data-image-publish-size-gib="${DATA_IMAGE_PUBLISH_SIZE_GIB}" \
        --partition-plan="${PARTITION_PLAN}" \
        --ovf-template="/bypass/variants/${VARIANT_NAME}/template.ovf" \
        --guest-images="${GUEST_IMAGES}" \
        ${XFS_DATA_PARTITION:+--with-xfs-data-partition=yes} \
        ${EROFS_ROOT_PARTITION:+--with-erofs-root-partition=yes} \
        ${UEFI_SECURE_BOOT:+--with-uefi-secure-boot=yes} \
        ${IN_PLACE_UPDATES:+--with-in-place-updates=yes} \
        ${ENCRYPTED_STORAGE:+--with-encrypted-storage=yes} \
        --with-standalone-image="${STANDALONE_IMAGE:-no}" ; \
    fi && \
    rm -rf /local/rpms && \
    chown -R "${BUILDER_UID}:${BUILDER_UID}" /output/ && \
    rm /output && \
    rm /bypass && \
    touch /tmp/.${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Creates an archive of the datastore migrations.
FROM repobuild AS migrationbuild
ARG ARCH
ARG VERSION_ID
ARG BUILD_ID
ARG NOCACHE
ARG VARIANT
ARG VARIANT_NAME=${VARIANT}
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
ARG BUILDER_UID
ARG IN_PLACE_UPDATES
ENV VARIANT=${VARIANT_NAME} VERSION_ID=${VERSION_ID} BUILD_ID=${BUILD_ID}
WORKDIR /root

USER root
RUN --mount=target=/host \
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    mkdir -p /local/migrations; \
    if [[ -n "${IN_PLACE_UPDATES}" ]]; then \
        find /bypass/build/rpms/ -maxdepth 2 -type f \
            -name "bottlerocket-migrations-*.rpm" \
            -not -iname '*debuginfo*' \
            -exec cp '{}' '/local/migrations/' ';' && \
        /host/build/tools/rpm2migrations \
            --package-dir=/local/migrations \
            --output-dir=/output; \
    else \
        mkdir -p /output/${VERSION_ID}-${BUILD_ID} && \
        touch /output/${VERSION_ID}-${BUILD_ID}/.no-migrations; \
    fi; \
    chown -R "${BUILDER_UID}:${BUILDER_UID}" /output && \
    rm -rf /local/migrations && \
    rm /output && \
    rm /bypass && \
    touch /tmp/.${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Creates an archive of kernel development sources and toolchain.
FROM repobuild AS kmodkitbuild
# The list of packages from the variant Cargo.toml package.metadata.build-variant.packages section.
ARG PACKAGES
ARG ARCH
ARG VERSION_ID
ARG BUILD_ID
ARG NOCACHE
ARG VARIANT
ARG VARIANT_NAME=${VARIANT}
ENV VARIANT=${VARIANT_NAME} VERSION_ID=${VERSION_ID} BUILD_ID=${BUILD_ID}
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
ARG BUILDER_UID
ARG EXTERNAL_KMOD_DEVELOPMENT

USER root

WORKDIR /tmp
RUN --mount=target=/host \
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    mkdir -p /local/archives; \
    if [[ -n "${EXTERNAL_KMOD_DEVELOPMENT}" ]]; then \
        KERNEL="$(printf "%s\n" ${PACKAGES} | awk '/^kernel-/{print $1}')" && \
        find /bypass/build/ -type f \
            -name "bottlerocket-${KERNEL}-archive-*.${ARCH}.rpm" \
            -exec cp '{}' '/local/archives/' ';' && \
        /host/build/tools/rpm2kmodkit \
            --archive-dir=/local/archives \
            --toolchain-dir=/toolchain \
            --output-dir=/output; \
    else \
        mkdir -p /output/${VERSION_ID}-${BUILD_ID} && \
        touch /output/${VERSION_ID}-${BUILD_ID}/.no-kmod-kit; \
    fi; \
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
FROM sdk AS imgrepack
ARG ARCH
ARG VERSION_ID
ARG BUILD_ID
ARG NOCACHE
ARG BYPASS_SOCKET
ARG OUTPUT_SOCKET
ARG BUILDER_UID
ARG VARIANT
ARG VARIANT_NAME=${VARIANT}
ARG VARIANT_PLATFORM
ARG IMAGE_NAME
ARG IMAGE_FORMAT
ARG OS_IMAGE_SIZE_GIB
ARG DATA_IMAGE_SIZE_GIB
ARG PARTITION_PLAN
ARG OS_IMAGE_PUBLISH_SIZE_GIB
ARG DATA_IMAGE_PUBLISH_SIZE_GIB
ARG UEFI_SECURE_BOOT
ARG EROFS_ROOT_PARTITION
ARG IN_PLACE_UPDATES
ARG STANDALONE_IMAGE
ENV VARIANT=${VARIANT_NAME} VARIANT_PLATFORM=${VARIANT_PLATFORM} \
    VERSION_ID=${VERSION_ID} BUILD_ID=${BUILD_ID}
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
    --mount=type=secret,id=efi-vars.json,target=/root/sbkeys/efi-vars.json \
    --mount=type=secret,id=kms-sign.json,target=/root/.config/aws-kms-pkcs11/config.json \
    --mount=type=secret,id=aws-access-key-id.env,target=/root/.aws/aws-access-key-id.env \
    --mount=type=secret,id=aws-secret-access-key.env,target=/root/.aws/aws-secret-access-key.env \
    --mount=type=secret,id=aws-session-token.env,target=/root/.aws/aws-session-token.env \
    /host/build/tools/pipesys link --fd-socket "${BYPASS_SOCKET}" --target /bypass && \
    /host/build/tools/pipesys link --fd-socket "${OUTPUT_SOCKET}" --target /output && \
    rm -rf /output/* && \
    /host/build/tools/img2img \
      --input-dir="/bypass/build/images/${ARCH}-${VARIANT_NAME}/${VERSION_ID}-${BUILD_ID}" \
      --output-dir=/output \
      --output-fmt="${IMAGE_FORMAT}" \
      --os-image-size-gib="${OS_IMAGE_SIZE_GIB}" \
      --data-image-size-gib="${DATA_IMAGE_SIZE_GIB}" \
      --os-image-publish-size-gib="${OS_IMAGE_PUBLISH_SIZE_GIB}" \
      --data-image-publish-size-gib="${DATA_IMAGE_PUBLISH_SIZE_GIB}" \
      --partition-plan="${PARTITION_PLAN}" \
      --ovf-template="/bypass/variants/${VARIANT_NAME}/template.ovf" \
      ${EROFS_ROOT_PARTITION:+--with-erofs-root-partition=yes} \
      ${UEFI_SECURE_BOOT:+--with-uefi-secure-boot=yes} \
      ${IN_PLACE_UPDATES:+--with-in-place-updates=yes} \
      --with-standalone-image="${STANDALONE_IMAGE:-no}" && \
    chown -R "${BUILDER_UID}:${BUILDER_UID}" /output/ && \
    rm /output && \
    rm /bypass && \
    touch /tmp/.${NOCACHE}

# =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=
# Finish up the image repack stage.
FROM scratch AS repack
ARG NOCACHE
COPY --from=imgrepack /tmp/.${NOCACHE} /output/
