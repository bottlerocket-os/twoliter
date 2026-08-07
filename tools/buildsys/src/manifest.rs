/*!
# Build system metadata

This module provides deserialization and convenience methods for build system
metadata located in `Cargo.toml`.

Cargo ignores the `package.metadata` table in its manifest, so it can be used
to store configuration for other tools. We recognize the following keys.

## Metadata for packages

`source-groups` is a list of directories in the top-level `sources` directory,
each of which contains a set of related Rust projects. Changes to files in
these groups should trigger a rebuild.
```ignore
[package.metadata.build-package]
source-groups = ["api"]
```

`external-files` is a list of out-of-tree files that should be retrieved
as additional dependencies for the build. If the path for the external
file name is not provided, it will be taken from the last path component
of the URL.
```ignore
[[package.metadata.build-package.external-files]]
path = "foo"
url = "https://foo"
sha512 = "abcdef"

[[package.metadata.build-package.external-files]]
path = "bar"
url = "https://bar"
sha512 = "123456"
```

The `bundle-*` keys on `external-files` are a group of optional modifiers
and are used to untar an upstream external file archive, vendor any dependent
code, and produce an additional archive with those dependencies.
Only `bundle-modules` is required when bundling an archive's dependences.

`bundle-modules` is a list of module "paradigms" the external-file should
be vendored through. For example, if a project contains a `go.mod` and `go.sum`
file, adding "go" to the list will vendor the dependencies through go modules.
Currently, only "go" is supported.

`bundle-root-path` is an optional argument that provides the filepath
within the archive that contains the module. By default, the first top level
directory in the archive is used. So, for example, given a Go project that has
the necessary `go.mod` and `go.sum` files in the archive located at the
filepath `a/b/c`, this `bundle-root-path` value should be "a/b/c". Or, given an
archive with a single directory that contains a Go project that has `go.mod`
and `go.sum` files located in that top level directory, this option may be
omitted since the single top-level directory will authomatically be used.

`bundle-output-path` is an optional argument that provides the desired path of
the output archive. By default, this will use the name of the existing archive,
but prepended with "bundled-". For example, if "my-unique-archive-name.tar.gz"
is entered as the value for `bundle-output-path`, then the output directory
will be named `my-unique-archive-name.tar.gz`. Or, by default, given the name
of some upstream archive is "my-package.tar.gz", the output archive would be
named `bundled-my-package.tar.gz`. This output path may then be referenced
within an RPM spec or when creating a package in order to access the vendored
upstream dependencies during build time.
```ignore
[[package.metadata.build-package.external-files]]
path = "foo"
url = "https://foo"
sha512 = "abcdef"
bundle-modules = [ "go" ]
bundle-root-path = "path/to/module"
bundle-output-path = "path/to/output.tar.gz"
```

`package-name` lets you override the package name in Cargo.toml; this is useful
if you have a package with "." in its name, for example, which Cargo doesn't
allow.  This means the directory name and spec file name can use your preferred
naming.
```ignore
[package.metadata.build-package]
package-name = "better.name"
```

`releases-url` is ignored by buildsys, but can be used by packager maintainers
to indicate a good URL for checking whether the software has had a new release.
```ignore
[package.metadata.build-package]
releases-url = "https://www.example.com/releases"
```

## Metadata for kits

When building a kit, it is necessary to include a `package.metadata.build-kit` key even though there
are no additional keys or attributes to add. This tells `buildsys` that the Cargo package is a kit.

For example:

```toml
[package]
name = "my-kit"
version = "0.1.0"

[package.metadata.build-kit]

[build-dependencies]
another-kit = { path = "../../kits/another-kit" }
some-package = { path = "../../packages/some-package" }
```

## Metadata for variants

`included-packages` is a list of packages that should be included in a variant.
```ignore
[package.metadata.build-variant]
included-packages = ["release"]
```

`image-format` is the desired format for the built images.
This can be `raw` (the default), `vmdk`, `qcow2`, or `eif`
(AWS Nitro Enclaves Image Format).

When `image-format = "eif"`, the image pipeline switches to `rpm2eif`,
which produces a dm-verity-protected, single-bank sidecar EIF plus a
minimal GPT disk image and a bare kernel. EIF is inherently a
stripped-down layout — no BOTTLEROCKET-DATA, PRIVATE, or RESERVED
partitions, no second bank of OS partitions, no in-place updates — so
the following features are automatically dropped from the silent
default seed for an EIF variant: `in-place-updates`,
`host-containers`. The following combinations are rejected at build
time (in the same style as `standalone-image = true`):
`uefi-secure-boot`, `encrypted-storage`, `in-place-updates`,
`host-containers`, and `partition-plan = "split"`. The
`os-image-size-gib` default is 1 GiB (vs. 2 GiB for a full image), and
the partition plan is forced to `unified`.
```ignore
[package.metadata.build-variant]
image-format = "vmdk"
```

`image-layout` is the desired layout for the built images.

`os-image-size-gib` is the desired size of the "os" disk image in GiB.
The specified size will be automatically divided into two banks, where each
bank contains the set of partitions needed for in-place upgrades. Roughly 40%
will be available for each root filesystem partition, with the rest allocated
to other essential system partitions.

`data-image-size-gib` is the desired size of the "data" disk image in GiB.
The full size will be used for the single data partition, except for the 2 MiB
overhead for the GPT labels and partition alignment. The data partition will be
automatically resized to fill the disk on boot, so it is usually not necessary
to increase this value.

`publish-image-size-hint-gib` is the desired size of the published image in GiB.
When the `split` layout is used, the "os" image volume will remain at the built
size, and any additional space will be allocated to the "data" image volume.
When the `unified` layout is used, this value will be used directly for the
single "os" image volume. The hint will be ignored if the combined size of the
"os" and "data" images exceeds the specified value.

`partition-plan` is the desired strategy for image partitioning.
This can be `split` (the default) for "os" and "data" images backed by separate
volumes, or `unified` to have "os" and "data" share the same volume.
```ignore
[package.metadata.build-variant.image-layout]
os-image-size-gib = 2
data-image-size-gib = 1
publish-image-size-hint-gib = 22
partition-plan = "split"
```

`supported-arches` is the list of architectures the variant is able to run on.
The values can be `x86_64` and `aarch64`.
If not specified, the variant can run on any of those architectures.
```ignore
[package.metadata.build-variant]
supported-arches = ["x86_64"]
```

`kernel-parameters` is a list of extra parameters to be added to the kernel command line.
The given parameters are inserted at the start of the command line.
```ignore
[package.metadata.build-variant]
kernel-parameters = [
   "console=ttyS42",
]
```

`eif-pcie-flags` overrides the PCIE flag word written into the EIF header for
`image-format = "eif"` variants. Accepted as either a TOML integer literal
(always decimal, per the TOML spec — e.g. `832`) or a `0x`-prefixed hex
string (`"0x340"`). Unprefixed string forms like `"340"` are rejected to
avoid ambiguity with the hex convention used on the `eif-builder --pcie-flags`
CLI, where `340` means `0x340`. Forwarded to `eif-builder --pcie-flags <hex>`.
When absent, `eif-builder`'s built-in default (`0x240`) is used. Non-EIF
variants ignore this field.

The header value **must** match the PCIE flags the sidecar shim passes to the
hypervisor at launch (`header == launch-flags`, enforced by the hypervisor);
this knob is the authoring surface for that value. See the doc comment on
`BuildVariant::eif_pcie_flags` for details.
```ignore
[package.metadata.build-variant]
image-format = "eif"
eif-pcie-flags = 0x340
```

`eif-kernel-format` selects the x86_64 kernel image format that `rpm2eif`
embeds into the EIF kernel section (and publishes as the sidecar
`${prefix}-kernel` artifact). Accepted values:

* `"bzimage"` (default): use the RPM-shipped compressed `vmlinuz` as-is.
  The Nitro Enclaves in-memory image loader that boots the sidecar EIF
  validates `BZIMAGE_HEADER_MAGIC` and boots via the bzImage protocol
  (same as every `nitro-cli`-built EIF), so this is the correct format
  for the sidecar path.
* `"vmlinux"`: extract an uncompressed ELF `vmlinux` from `vmlinuz` and
  embed that. Only appropriate for a bare-metal Firecracker PVH loader.
  Kept selectable because Firecracker upstream has shipped conflicting
  guidance on compressed vs. uncompressed kernels; if a variant author
  needs to feed a PVH-only loader, this keeps the option available
  without having to fork `rpm2eif`.

Ignored on aarch64 (only the PE-wrapped `Image` format is valid there)
and on non-EIF variants.
```ignore
[package.metadata.build-variant]
image-format = "eif"
eif-kernel-format = "bzimage"
```

`image-features` is a map of image feature flags, which can be enabled or disabled. This allows us
to conditionally use or exclude certain image-level features in variants.

`in-place-updates` means that the disk layout for the variant will support in-place updates, which
requires a parallel set of partition table entries to use as the active and passive banks. For
backwards compatibility, this feature is enabled by default unless explicitly disabled.
```ignore
[package.metadata.build-variant.image-features]
in-place-updates = true
```

`host-containers` means that software support for host and bootstrap containers will be included in
the variant. This provides a way to extend the host OS with additional software packaged as a
container, which can be run on boot or as a background service. For backwards compatibility, this
feature is enabled by default unless explicitly disabled.
```ignore
[package.metadata.build-variant.image-features]
host-containers = true
```

`grub-set-private-var` means that the grub image for the current variant includes the command to
find the BOTTLEROCKET_PRIVATE partition and set the appropriate `$private` variable for the grub
config file to consume. This feature flag is a prerequisite for Boot Config support.
```ignore
[package.metadata.build-variant.image-features]
grub-set-private-var = true
```

`systemd-networkd` uses the `systemd-networkd` network backend in place of `wicked`.  This feature
flag is meant primarily for development, and will be removed when development has completed.
```ignore
[package.metadata.build-variant.image-features]
systemd-networkd = true
```

`xfs-data-partition` changes the filesystem for the data partition from ext4 to xfs. The
default will remain ext4 and xfs is opt-in.

```ignore
[package.metadata.build-variant.image-features]
xfs-data-partition = true
```

`erofs-root-partition` changes the filesystem for the root partition from ext4 to erofs. The
default will remain ext4 and erofs is opt-in.

```ignore
[package.metadata.build-variant.image-features]
erofs-root-partition = true
```

`uefi-secure-boot` means that the bootloader and kernel are signed. The grub image for the current
variant will have a public GPG baked in, and will expect the grub config file to have a valid
detached signature. Published artifacts such as AMIs and OVAs will enforce the signature checks
when the platform supports it.

```ignore
[package.metadata.build-variant.image-features]
uefi-secure-boot = true
```

`fips` means that FIPS-certified modules will be used for cryptographic operations. This affects
the kernel at runtime. It also causes alternate versions of Go and Rust programs that use
FIPS-compliant ciphers to be included in the image.

```ignore
[package.metadata.build-variant.image-features]
fips = true
```

`external-kmod-development` enables functionality for building custom kernel modules at runtime

```ignore
[package.metadata.build-variant.image-features]
external-kmod-development = true
```

`encrypted-storage` enables encryption for writable filesystems and block devices. This should
only be enabled for variants that can guarantee that a TPM 2.0 device will be present at runtime.

```ignore
[package.metadata.build-variant.image-features]
encrypted-storage = true
```

`standalone-image` controls whether the image is a stripped-down OS without the
Bottlerocket-provided software stack. The default is `false` (full Bottlerocket image).

When `standalone-image = true`, the Bottlerocket-provided software stack — the orchestrator
integration (kubelet, containerd, host-containers, admin-containers), the settings system
(apiserver, datastore, settings rendering, migrations), in-place updates, and the
BOTTLEROCKET-PRIVATE / BOTTLEROCKET-DATA partitions — is NOT built into the image. The image
is reduced to the kernel, base userspace, dm-verity-protected rootfs, and the update mechanism.
The OS image has a single bank of OS partitions (BIOS-BOOT, EFI-SYSTEM, BOOT-A, ROOT-A,
HASH-A) with no `RESERVED-A` partition; ROOT-A absorbs the slack. The default OS image size
becomes 1 GiB unless `os-image-size-gib` is explicitly set. The variant author owns all
system configuration; if Bottlerocket components that expect persistent storage at
`partlabel=BOTTLEROCKET-DATA` are shipped, such a volume may optionally be attached at
runtime.

`standalone-image = true` requires `partition-plan = "unified"`, and is incompatible with
`uefi-secure-boot`, `encrypted-storage`, `in-place-updates`, and `host-containers`; the build
will fail fast in those cases. Setting `standalone-image = true` also turns off the silent
defaults for `in-place-updates` and `host-containers` so that, once `partition-plan = "unified"`
is set, toggling `standalone-image = true` is sufficient to produce a stripped-down image.

```ignore
[package.metadata.build-variant.image-features]
standalone-image = true

[package.metadata.build-variant.image-layout]
partition-plan = "unified"
```

*/

mod error;

use crate::BuildType;
use buildsys_config::EXTERNAL_KIT_METADATA;
use guppy::graph::{DependencyDirection, PackageGraph, PackageLink, PackageMetadata};
use guppy::{CargoMetadata, PackageId};
use serde::{de, Deserialize, Deserializer, Serialize};
use snafu::{ensure, OptionExt, ResultExt, Snafu};
use std::cmp::max;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::convert::TryFrom;
use std::fmt::{self, Display};
use std::fs;
use std::fs::read;
use std::path::{Path, PathBuf};

#[derive(Debug, Snafu)]
pub struct Error(error::Error);
type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Manifest {
    graph: PackageGraph,
    manifest_info: ManifestInfo,
}

impl Manifest {
    /// Extract the settings we understand from `Cargo.toml` and construct a dependency graph.
    pub fn new(manifest: impl AsRef<Path>, cargo_metadata: impl AsRef<Path>) -> Result<Self> {
        let manifest_info = ManifestInfo::new(manifest)?;
        let cargo_metadata = cargo_metadata.as_ref();
        let cargo_metadata_json_contents =
            fs::read_to_string(cargo_metadata).context(error::CargoMetadataReadSnafu {
                path: &cargo_metadata,
            })?;
        let graph = CargoMetadata::parse_json(cargo_metadata_json_contents)
            .context(error::CargoMetadataParseSnafu {
                path: cargo_metadata,
            })?
            .build_graph()
            .context(error::GraphBuildSnafu {
                path: cargo_metadata,
            })?;
        Ok(Self {
            manifest_info,
            graph,
        })
    }

    /// List all packages that are package dependencies. That is, follow all dependencies in the cargo
    /// dependency graph that lead to more packages, and do not follow those that involve kits. This
    /// gives a list of all the packages that are required when we are build a package, or all of the
    /// packages that should be included when building a kit.
    pub fn package_dependencies(&self) -> Result<Vec<String>> {
        let name = self.info().manifest_name();
        let manifest_type = self.info().build_type()?;
        let id = find_id(name, &self.graph, manifest_type)
            .context(error::RootDependencyMissingSnafu { name })?;
        let ids = [&id];
        let query = self
            .graph
            .query_forward(ids)
            .context(error::CargoPackageQuerySnafuSnafu { id })?;
        let package_set = query.resolve_with_fn(|_, link| {
            let to = link.to();
            is_valid_dep(name, &link) && is_manifest_type(&to, BuildType::Package)
        });
        let mut packages: Vec<String> = package_set
            .packages(DependencyDirection::Forward)
            .filter_map(|pkg_metadata| filter_map_to_name(name, &pkg_metadata))
            .collect();

        // Sort so that this function has consistent, dependable output regardless of graph internals.
        packages.sort();
        Ok(packages)
    }

    /// List all kits needed for the build.
    pub fn kit_dependencies(&self) -> Result<Vec<String>> {
        let name = self.info().manifest_name();
        let manifest_type = self.info().build_type()?;
        let id = find_id(name, &self.graph, manifest_type)
            .context(error::RootDependencyMissingSnafu { name })?;
        let ids = [&id];
        let query = self
            .graph
            .query_forward(ids)
            .context(error::CargoPackageQuerySnafuSnafu { id })?;
        let package_set = query.resolve();
        let mut kits: Vec<String> = package_set
            .packages(DependencyDirection::Forward)
            .filter(|pkg_metadata| is_manifest_type(pkg_metadata, BuildType::Kit))
            .filter_map(|pkg_metadata| filter_map_to_name(name, &pkg_metadata))
            .collect();
        kits.sort();
        Ok(kits)
    }

    /// For a host variant manifest, validate the `guest-images` field and return the resolved
    /// list of `(guest_name, install_path)` pairs. Each guest must:
    ///
    /// 1. Be declared as a **direct** `[build-dependencies]` entry of this host variant (so
    ///    Cargo builds it before the host's `build.rs` runs, and so the relationship is
    ///    explicit rather than relying on a transitive path).
    /// 2. Itself be a variant crate (carry `[package.metadata.build-variant]`).
    /// 3. Have an absolute install-path with no `..` components.
    /// 4. Not name the host variant itself.
    /// 5. Use a name and path free of `:` and newline characters, which are reserved as
    ///    field/record separators in the `GUEST_IMAGES` build-arg passed to the image stage.
    ///
    /// Returns an empty vector if the manifest declares no `guest-images`.
    pub fn guest_image_variant_deps(&self) -> Result<Vec<(String, PathBuf)>> {
        let Some(guest_images) = self.info().guest_images() else {
            return Ok(Vec::new());
        };
        if guest_images.is_empty() {
            return Ok(Vec::new());
        }

        let host_name = self.info().manifest_name();

        // Syntactic validation runs first, before any cargo-graph work, so configuration
        // errors fail fast and are unit-testable without a graph fixture.
        validate_guest_image_entries(host_name, guest_images, self.info().image_format())?;

        // Collect the set of *direct* build-dependency variant names of the host. We do not
        // walk the graph transitively: each guest must be an explicit build-dep of the host so
        // the relationship is visible in `Cargo.toml`.
        let id = find_id(host_name, &self.graph, BuildType::Variant)
            .context(error::RootDependencyMissingSnafu { name: host_name })?;
        let host_metadata = self
            .graph
            .metadata(&id)
            .context(error::CargoPackageQuerySnafuSnafu { id: id.clone() })?;
        let direct_build_dep_variants: HashSet<String> = host_metadata
            .direct_links()
            .filter(|link| link.build().is_present())
            .map(|link| link.to())
            .filter(|to| is_manifest_type(to, BuildType::Variant))
            .filter_map(|to| filter_map_to_name(host_name, &to))
            .collect();

        // To produce a more actionable error, also gather every *transitively reachable*
        // variant so we can distinguish "missing" from "transitive only".
        let ids = [&id];
        let query = self
            .graph
            .query_forward(ids)
            .context(error::CargoPackageQuerySnafuSnafu { id: id.clone() })?;
        let package_set = query.resolve_with_fn(|_, link| is_valid_dep(host_name, &link));
        let reachable_variants: HashSet<String> = package_set
            .packages(DependencyDirection::Forward)
            .filter(|pkg_metadata| is_manifest_type(pkg_metadata, BuildType::Variant))
            .filter_map(|pkg_metadata| filter_map_to_name(host_name, &pkg_metadata))
            .collect();

        let mut resolved = Vec::with_capacity(guest_images.len());
        for (guest, path) in guest_images.iter() {
            if !direct_build_dep_variants.contains(guest) {
                if reachable_variants.contains(guest) {
                    return Err(error::GuestImagesNotDirectBuildDepSnafu {
                        name: host_name.to_string(),
                        guest: guest.clone(),
                    }
                    .build()
                    .into());
                }
                return Err(error::GuestImagesMissingBuildDepSnafu {
                    name: host_name.to_string(),
                    guest: guest.clone(),
                }
                .build()
                .into());
            }
            resolved.push((guest.clone(), path.clone()));
        }
        Ok(resolved)
    }

    pub fn info(&self) -> &ManifestInfo {
        &self.manifest_info
    }
}

#[derive(Deserialize, Debug)]
pub struct ExternalKitMetadataView {
    #[serde(rename = "kit")]
    kits: Vec<ImageView>,
    project_vendor: String,
}

impl ExternalKitMetadataView {
    /// Load a view of the external kit metadata
    pub fn load<P>(path: P) -> Result<Self>
    where
        P: AsRef<Path>,
    {
        let metadata_file = path.as_ref().join(EXTERNAL_KIT_METADATA);
        let metadata_bytes =
            read(&metadata_file).context(error::ExternalKitMetadataFileReadSnafu {
                path: metadata_file.clone(),
            })?;
        serde_json::from_slice(metadata_bytes.as_slice())
            .context(error::ExternalKitMetadataLoadSnafu {
                path: metadata_file.clone(),
            })
            .map_err(Error)
    }
    /// List all external kits needed for the build in the format of "<vendor>-<kit_name>"
    pub fn list(&self) -> Vec<String> {
        self.kits
            .iter()
            .map(|x| format!("{}/{}", x.vendor, x.name))
            .collect()
    }

    pub fn get_project_vendor(&self) -> &str {
        self.project_vendor.as_ref()
    }
}

#[derive(Deserialize, Debug)]
struct ImageView {
    name: String,
    vendor: String,
}

/// The nested structures here are somewhat complex, but they make it trivial
/// to deserialize the structure we expect to find in the manifest.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct ManifestInfo {
    package: Package,
}

impl ManifestInfo {
    /// Extract the settings we understand from `Cargo.toml`.
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        let manifest_data =
            fs::read_to_string(path).context(error::ManifestFileReadSnafu { path })?;
        let manifest_info: ManifestInfo =
            toml::from_str(&manifest_data).context(error::ManifestFileLoadSnafu { path })?;
        Ok(manifest_info)
    }

    pub fn manifest_name(&self) -> &str {
        &self.package.name
    }

    /// Convenience method to return the list of source groups.
    pub fn source_groups(&self) -> Option<&Vec<PathBuf>> {
        self.build_package().and_then(|b| b.source_groups.as_ref())
    }

    /// Convenience method to return the list of external files.
    pub fn external_files(&self) -> Option<&Vec<ExternalFile>> {
        self.build_package().and_then(|b| b.external_files.as_ref())
    }

    /// Convenience method to return the package name. If the manifest has an override in the
    /// `package.metadata.build-package.package-name` key, it is returned, otherwise the
    /// Cargo manifest name is returned from `package.name`.
    pub fn package_name(&self) -> &str {
        self.build_package()
            .and_then(|b| b.package_name.as_deref())
            .unwrap_or_else(|| self.manifest_name())
    }

    /// Convenience method to return the kit name. If the manifest has an override in the
    /// `package.metadata.build-kit.kit-name` key, it is returned, otherwise the Cargo manifest name
    /// is returned from `package.name`.
    pub fn kit_name(&self) -> &str {
        self.build_kit()
            .and_then(|b| b.kit_name.as_deref())
            .unwrap_or_else(|| self.manifest_name())
    }

    /// Convenience method to return the kit vendor
    pub fn kit_vendor(&self) -> Result<String> {
        Ok(self
            .build_kit()
            .context(error::NoKitVendorSnafu {
                id: self.package.name.clone(),
            })?
            .vendor
            .clone())
    }

    /// Convenience method to find whether the package is sensitive to variant changes.
    pub fn variant_sensitive(&self) -> Option<&VariantSensitivity> {
        self.build_package()
            .and_then(|b| b.variant_sensitive.as_ref())
    }

    /// Convenience method to return the image features tracked by this package.
    pub fn package_features(&self) -> Option<HashSet<&ImageFeature>> {
        self.build_package()
            .and_then(|b| b.package_features.as_ref().map(|m| m.iter().collect()))
    }

    /// Convenience method to return the list of included packages.
    pub fn included_packages(&self) -> Option<&Vec<String>> {
        self.build_variant()
            .and_then(|b| b.included_packages.as_ref())
    }

    /// Convenience method to return the image format override, if any.
    pub fn image_format(&self) -> Option<&ImageFormat> {
        self.build_variant().and_then(|b| b.image_format.as_ref())
    }

    /// Convenience method to return the image layout, if specified.
    pub fn image_layout(&self) -> Option<&ImageLayout> {
        self.build_variant().map(|b| &b.image_layout)
    }

    /// Convenience method to return the supported architectures for this variant.
    pub fn supported_arches(&self) -> Option<&HashSet<SupportedArch>> {
        self.build_variant()
            .and_then(|b| b.supported_arches.as_ref())
    }

    /// Convenience method to return the kernel parameters for this variant.
    pub fn kernel_parameters(&self) -> Option<&Vec<String>> {
        self.build_variant()
            .and_then(|b| b.kernel_parameters.as_ref())
    }

    /// Convenience method to return the EIF header PCIE flags override for
    /// this variant. Only meaningful when `image-format = "eif"`; ignored
    /// otherwise.
    pub fn eif_pcie_flags(&self) -> Option<u16> {
        self.build_variant().and_then(|b| b.eif_pcie_flags)
    }

    /// Convenience method to return the x86_64 EIF kernel format override
    /// for this variant. Only meaningful when `image-format = "eif"` on
    /// x86_64; ignored otherwise.
    pub fn eif_kernel_format(&self) -> Option<EifKernelFormat> {
        self.build_variant().and_then(|b| b.eif_kernel_format)
    }

    /// Convenience method to return the enabled image features for this variant.
    pub fn image_features(&self) -> Option<HashSet<ImageFeature>> {
        let variant = self.build_variant()?;
        // If the user explicitly enabled `standalone-image`, drop the silent
        // defaults for `in-place-updates` and `host-containers` since they
        // are incompatible with the stripped-down image. An explicit
        // `in-place-updates = true` is still rejected later by the validator.
        //
        // `image-format = "eif"` is treated the same as `standalone-image =
        // true` for seeding purposes: an EIF is inherently a stripped-down
        // single-bank image, so shipping the second bank / host-containers /
        // BOTTLEROCKET-DATA subsystems by default only invites silent
        // misconfiguration when the Dockerfile drops those flags on the way
        // to `rpm2eif`. The validator will still reject an *explicit*
        // conflicting feature set with a clear error.
        let standalone_image_explicitly_enabled = variant
            .image_features
            .as_ref()
            .and_then(|m| m.get(&ImageFeature::StandaloneImage))
            .copied()
            .unwrap_or(false);
        let is_eif = matches!(variant.image_format, Some(ImageFormat::Eif));
        let mut features = if standalone_image_explicitly_enabled || is_eif {
            HashSet::from([
                ImageFeature::StandaloneImage,
                ImageFeature::ExternalKmodDevelopment,
            ])
        } else {
            HashSet::from([
                ImageFeature::InPlaceUpdates,
                ImageFeature::HostContainers,
                ImageFeature::ExternalKmodDevelopment,
            ])
        };
        if let Some(image_features) = &variant.image_features {
            for (feature, enabled) in image_features.iter() {
                if *enabled {
                    features.insert(*feature);
                } else {
                    features.remove(feature);
                }
            }
        }
        for experiment in EXPERIMENTAL_IMAGE_FEATURES {
            if features.contains(experiment) {
                println!("cargo:warning=Image feature {experiment} is experimental; use at your own risk!");
            }
        }
        for deprecated in DEPRECATED_IMAGE_FEATURES {
            if features.contains(deprecated) {
                println!("cargo:warning=Image feature {deprecated} is deprecated and will be removed in a future release!");
            }
        }
        Some(features)
    }

    /// Convenience method to return the guest-image install paths declared by this variant.
    pub fn guest_images(&self) -> Option<&BTreeMap<String, PathBuf>> {
        self.build_variant().and_then(|b| b.guest_images.as_ref())
    }

    /// Returns the type of build the manifest is requesting.
    // TODO - alter ManifestInfo struct to use an enum and eliminate the use of Result here.
    pub fn build_type(&self) -> Result<BuildType> {
        if self.build_package().is_some() {
            Ok(BuildType::Package)
        } else if self.build_kit().is_some() {
            Ok(BuildType::Kit)
        } else if self.build_variant().is_some() {
            Ok(BuildType::Variant)
        } else {
            println!(
                "cargo::warning=Expected to find one of 'build-package', 'build-kit', or \
                'build-variant' in package.metadata. Assuming 'build-package'."
            );
            Ok(BuildType::Package)
        }
    }

    /// Helper methods to navigate the series of optional struct fields.
    fn build_package(&self) -> Option<&BuildPackage> {
        self.package
            .metadata
            .as_ref()
            .and_then(|m| m.build_package.as_ref())
    }

    fn build_kit(&self) -> Option<&BuildKit> {
        self.package
            .metadata
            .as_ref()
            .and_then(|m| m.build_kit.as_ref())
    }

    fn build_variant(&self) -> Option<&BuildVariant> {
        self.package
            .metadata
            .as_ref()
            .and_then(|m| m.build_variant.as_ref())
    }

    /// Returns the names of any sibling `[package.metadata.build-*]` tables present alongside
    /// this manifest's primary build metadata. Used to detect ambiguous manifests that mix
    /// build types.
    pub fn conflicting_build_metadata(&self) -> Vec<&'static str> {
        let mut conflicts = Vec::new();
        if self.build_package().is_some() {
            conflicts.push("build-package");
        }
        if self.build_kit().is_some() {
            conflicts.push("build-kit");
        }
        if self.build_variant().is_some() {
            conflicts.push("build-variant");
        }
        conflicts
    }

    /// Returns variant attribute overrides from `[package.metadata.build-variant]`.
    pub fn variant_overrides(&self) -> bottlerocket_variant::VariantOverrides {
        self.build_variant()
            .map(|bv| bottlerocket_variant::VariantOverrides {
                platform: bv.platform.clone(),
                runtime: bv.runtime.clone(),
                family: bv.family.clone(),
                version: bv.version.clone(),
                flavor: bv.flavor.clone(),
            })
            .unwrap_or_default()
    }
}

/// Syntactic validation of the `[package.metadata.build-variant.guest-images]` map for the
/// host variant `host_name`. Performs only the checks that don't require the cargo graph:
/// image-format compatibility, self-reference, name well-formedness, path absoluteness, and
/// path safety. Caller is responsible for the graph-level "must be a direct build-dep" check.
///
/// The `:` and newline checks exist because the `GUEST_IMAGES` build-arg passed to the image
/// stage is a newline-delimited list of `<guest>:<install_path>:<host_image_dir>` triples; a
/// literal `:` or newline in either field would corrupt parsing in `rpm2img`. The `..`
/// rejection prevents an install path from escaping the host rootfs target.
///
/// Guest-image embedding is implemented only in `rpm2img`; the `rpm2eif` pipeline does not
/// consume the `GUEST_IMAGES` build-arg. Rejecting `image-format = "eif"` here turns a
/// silent "build succeeds, guest images not embedded" footgun into a clear build-time error,
/// mirroring the existing `EifRepackUnsupported` check for `img2img`.
fn validate_guest_image_entries(
    host_name: &str,
    guest_images: &BTreeMap<String, PathBuf>,
    image_format: Option<&ImageFormat>,
) -> Result<()> {
    ensure!(
        !matches!(image_format, Some(ImageFormat::Eif)),
        error::GuestImagesUnsupportedImageFormatSnafu {
            name: host_name.to_string(),
        }
    );
    for (guest, path) in guest_images.iter() {
        ensure!(
            guest != host_name,
            error::GuestImagesSelfReferenceSnafu {
                name: host_name.to_string(),
            }
        );
        ensure!(
            !guest.is_empty() && !guest.contains(':') && !guest.contains('\n'),
            error::GuestImagesInvalidNameSnafu {
                name: host_name.to_string(),
                guest: guest.clone(),
            }
        );
        ensure!(
            path.is_absolute(),
            error::GuestImagesPathNotAbsoluteSnafu {
                name: host_name.to_string(),
                guest: guest.clone(),
                path: path.clone(),
            }
        );
        let path_str = path.to_string_lossy();
        let has_invalid_char = path_str.contains(':') || path_str.contains('\n');
        let has_parent_component = path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir));
        ensure!(
            !has_invalid_char && !has_parent_component,
            error::GuestImagesInvalidPathSnafu {
                name: host_name.to_string(),
                guest: guest.clone(),
                path: path.clone(),
            }
        );
    }
    Ok(())
}

/// For the "top-level manifest", i.e. the thing that `buildsys` is building, only
/// `build-dependencies` are valid. This is because we would need all artifacts before the top-level
/// manifest's `build.rs` runs. Once we go deeper in the graph, then both `build-dependencies` and
/// `dependencies` are valid because they would be built in time for the top-level `build.rs`.
fn is_valid_dep(top_manifest_name: &str, link: &PackageLink<'_>) -> bool {
    let is_top_level_manifest = link.from().name() == top_manifest_name;
    let is_deeper_level_manifest = !is_top_level_manifest;
    is_deeper_level_manifest || link.build().is_present()
}

fn is_manifest_type(pkg_metadata: &PackageMetadata, manifest_type: BuildType) -> bool {
    let metadata_table = pkg_metadata.metadata_table();
    let has_metadata = metadata_table.get("build-package").is_some()
        || metadata_table.get("build-kit").is_some()
        || metadata_table.get("build-variant").is_some();

    match manifest_type {
        BuildType::Package => metadata_table.get("build-package").is_some() || !has_metadata,
        BuildType::Kit => metadata_table.get("build-kit").is_some(),
        BuildType::Variant => metadata_table.get("build-variant").is_some(),
        BuildType::Repack => unreachable!("Repacking is not defined in manifests"),
    }
}

fn find_id(name: &str, graph: &PackageGraph, manifest_type: BuildType) -> Option<PackageId> {
    for pkg_metadata in graph.packages() {
        if is_manifest_type(&pkg_metadata, manifest_type) && pkg_metadata.name() == name {
            return Some(pkg_metadata.id().to_owned());
        }
    }
    None
}

/// Lists include the "top-level manifest", i.e. the thing that `buildsys` is being asked to build.
/// We do not want this, we want only a list of things that it depends on. Here we convert
/// `PackageMetadata` objects to the `String` name, and filter out the "top-level manifest".
fn filter_map_to_name(top_manifest_name: &str, pkg_metadata: &PackageMetadata) -> Option<String> {
    if pkg_metadata.name() == top_manifest_name {
        None
    } else {
        // Return the package override name, if it exists, or else the Cargo manifest name.
        Some(get_buildsys_package_name(pkg_metadata))
    }
}

/// Get the `package.metadata.build-package.package-name` value if there is one, otherwise return
/// the Cargo manifest's package name. This is the same as `manifest_info.package_name()`.
fn get_buildsys_package_name(pkg_metadata: &PackageMetadata) -> String {
    let metadata = pkg_metadata.metadata_table();
    let package_name_override = metadata
        .get("build-package")
        .and_then(|v| v.as_object())
        .and_then(|build_package| build_package.get("package-name"))
        .and_then(|package_name| package_name.as_str());

    package_name_override
        .unwrap_or_else(|| pkg_metadata.name())
        .to_string()
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
struct Package {
    name: String,
    metadata: Option<Metadata>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
struct Metadata {
    build_package: Option<BuildPackage>,
    build_kit: Option<BuildKit>,
    build_variant: Option<BuildVariant>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub struct BuildPackage {
    pub external_files: Option<Vec<ExternalFile>>,
    pub package_name: Option<String>,
    pub releases_url: Option<String>,
    pub source_groups: Option<Vec<PathBuf>>,
    pub variant_sensitive: Option<VariantSensitivity>,
    pub package_features: Option<Vec<ImageFeature>>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub struct BuildKit {
    pub kit_name: Option<String>,
    pub vendor: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(untagged)]
pub enum VariantSensitivity {
    Any(bool),
    Specific(SensitivityType),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub enum SensitivityType {
    Platform,
    Runtime,
    Family,
    Flavor,
}

/// Deserialize a `u16` from either a TOML integer literal (`576`, always
/// decimal per the TOML spec) or a `0x`/`0X`-prefixed hex string (`"0x240"`).
///
/// Unprefixed string forms (e.g. `"240"`) are **rejected** rather than
/// silently interpreted. The rationale: PCIE flag values are conventionally
/// written in hex on the `eif-builder --pcie-flags` CLI (whose parser is
/// hex-only, prefix optional) and in the aws-nitro-enclaves-image-format
/// header definitions. A user copy-pasting a hex value like `340` from those
/// contexts into `eif-pcie-flags = "340"` almost certainly means `0x340`
/// (bits 6, 8, 9), not `340` decimal (`0x154`, bits 2, 4, 6, 8). Treating
/// the unprefixed string as decimal would produce a header/launch-flags
/// mismatch that only surfaces at enclave launch, which is the exact kind
/// of silent misconfiguration this knob exists to prevent. Forcing an
/// explicit base makes the author's intent unambiguous at parse time:
/// integer form is unambiguously decimal, string form is unambiguously hex.
fn deserialize_u16_hex_or_int<'de, D>(deserializer: D) -> std::result::Result<Option<u16>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Int(u64),
        Str(String),
    }

    let Some(value) = Option::<Repr>::deserialize(deserializer)? else {
        return Ok(None);
    };
    match value {
        Repr::Int(n) => u16::try_from(n).map(Some).map_err(|_| {
            <D::Error as de::Error>::custom(format!("value {n} does not fit in a u16"))
        }),
        Repr::Str(s) => {
            let trimmed = s.trim();
            let hex_body = trimmed
                .strip_prefix("0x")
                .or_else(|| trimmed.strip_prefix("0X"))
                .ok_or_else(|| {
                    <D::Error as de::Error>::custom(format!(
                        "invalid `eif-pcie-flags` value {s:?}: string form requires a `0x` prefix \
                         (e.g. \"0x340\") to make the base explicit; use a bare integer literal \
                         (e.g. `832`) for decimal"
                    ))
                })?;
            u16::from_str_radix(hex_body, 16)
                .map(Some)
                .map_err(|e| <D::Error as de::Error>::custom(format!("invalid hex u16 {s:?}: {e}")))
        }
    }
}

/// Metadata for a `build-variant`.
///
/// `guest-images` is a map from a guest variant's crate name to an absolute install path in
/// this (host) variant's root filesystem. During the host's image build stage, each guest's
/// `build/images/<arch>-<guest>/<version>/` directory is copied recursively into the install
/// path. The guest variant must be declared as a `[build-dependencies]` of this host variant
/// so that Cargo builds it first; otherwise the host build fails.
///
/// Example `Cargo.toml` snippet:
/// ```ignore
/// [package.metadata.build-variant]
/// included-packages = ["release"]
/// guest-images = { "inner-variant" = "/usr/share/bottlerocket/guests/inner" }
///
/// [build-dependencies]
/// inner-variant = { path = "../inner-variant" }
/// ```
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct BuildVariant {
    pub included_packages: Option<Vec<String>>,
    pub image_format: Option<ImageFormat>,
    #[serde(default)]
    pub image_layout: ImageLayout,
    pub supported_arches: Option<HashSet<SupportedArch>>,
    pub kernel_parameters: Option<Vec<String>>,
    pub image_features: Option<HashMap<ImageFeature, bool>>,
    /// EIF header PCIE flags override for `image-format = "eif"` variants.
    ///
    /// Encoded in the variant Cargo.toml as either a TOML integer literal
    /// (decimal, e.g. `832`) or a `0x`-prefixed hex string (e.g. `"0x340"`).
    /// Unprefixed string forms like `"340"` are rejected: the `eif-builder
    /// --pcie-flags` CLI treats `340` as hex, so silently reading the
    /// unprefixed string as decimal here would produce a header/launch-flags
    /// mismatch that only surfaces at enclave launch. Forwarded to
    /// `eif-builder --pcie-flags <hex>` at rpm2eif / eif2eif time. When
    /// absent, `eif-builder` uses its built-in `DEFAULT_PCIE_FLAGS`
    /// (`EIF_HDR_FLAG_PCIE | EIF_HDR_FLAG_PCIE_VIRTIO`, `0x240`). Non-EIF
    /// variants ignore this field.
    ///
    /// Contract: the flags written into the EIF header here **must** match the
    /// PCIE flags the sidecar shim passes to the hypervisor at launch. The
    /// hypervisor enforces `header == launch-flags`; a mismatch fails
    /// attestation / launch. Today the shim's launch flags are hard-coded on
    /// the shim side, so this knob is what lets a variant author the value
    /// that keeps the two sides in sync. Once the shim derives its launch
    /// flags from the header directly (host echoes, hypervisor enforces), the
    /// coupling goes away — but the knob is still how the header value gets
    /// authored per-variant.
    ///
    /// Example:
    /// ```ignore
    /// [package.metadata.build-variant]
    /// image-format = "eif"
    /// eif-pcie-flags = 0x340
    /// ```
    #[serde(default, deserialize_with = "deserialize_u16_hex_or_int")]
    pub eif_pcie_flags: Option<u16>,
    /// x86_64 kernel image format that `rpm2eif` embeds in the EIF kernel
    /// section (and publishes as the sidecar `${prefix}-kernel` artifact).
    ///
    /// Defaults to [`EifKernelFormat::Bzimage`] when unset: the sidecar
    /// enclave loader validates `BZIMAGE_HEADER_MAGIC` and boots via the
    /// bzImage protocol, matching every `nitro-cli`-built EIF. Kept
    /// selectable (rather than a hardcoded constant) because Firecracker
    /// upstream has flipped between compressed and uncompressed kernels
    /// more than once, and a bare-metal PVH loader still needs the ELF
    /// `vmlinux` form.
    ///
    /// Ignored on aarch64 (only PE-wrapped `Image` is valid there) and on
    /// non-EIF variants. Forwarded to `rpm2eif` as the
    /// `EIF_KERNEL_FORMAT` build-arg (empty string when unset; `rpm2eif`
    /// applies its own default).
    #[serde(default)]
    pub eif_kernel_format: Option<EifKernelFormat>,
    /// Map of guest variant crate name -> absolute install path in this variant's root
    /// filesystem.
    pub guest_images: Option<BTreeMap<String, PathBuf>>,
    // Variant attribute overrides
    pub platform: Option<String>,
    pub runtime: Option<String>,
    pub family: Option<String>,
    pub version: Option<String>,
    pub flavor: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Eif,
    Qcow2,
    Raw,
    Vmdk,
}

/// x86_64 kernel image format that `rpm2eif` embeds in the EIF kernel
/// section. See [`BuildVariant::eif_kernel_format`] for the rationale on
/// keeping this selectable rather than pinning one value.
///
/// The TOML values `"bzimage"` and `"vmlinux"` are accepted; both are
/// case-insensitive (via the `lowercase` rename). Ignored on aarch64.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EifKernelFormat {
    /// Compressed `vmlinuz` embedded verbatim. Matches the boot protocol
    /// the sidecar Nitro Enclaves in-memory image loader expects (and
    /// every `nitro-cli`-built EIF).
    Bzimage,
    /// Uncompressed ELF `vmlinux` extracted from `vmlinuz`. For a
    /// bare-metal Firecracker PVH loader.
    Vmlinux,
}

impl EifKernelFormat {
    /// String form matching the TOML value and the `EIF_KERNEL_FORMAT`
    /// build-arg / `--eif-kernel-format` CLI value expected by `rpm2eif`.
    pub fn as_str(&self) -> &'static str {
        match self {
            EifKernelFormat::Bzimage => "bzimage",
            EifKernelFormat::Vmlinux => "vmlinux",
        }
    }
}

#[derive(Deserialize, Debug, Copy, Clone)]
/// Constrain specified image sizes to a plausible range, from 0 - 65535 GiB.
pub struct ImageSize(u16);

impl Display for ImageSize {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Deserialize, Debug, Copy, Clone)]
#[serde(rename_all = "kebab-case")]
/// Image layout settings as deserialized from a variant manifest.
///
/// Note: callers should always run the layout through
/// [`resolved_image_layout`] before using it to drive a build. Other fields
/// (`data_image_size_gib`, `partition_plan`) remain `pub` for direct access
/// since they have no feature-dependent default.
pub struct ImageLayout {
    /// Raw value as it appeared (or did not appear) in the manifest. Use
    /// [`ImageLayout::os_image_size_gib`] to read the resolved value, which
    /// substitutes the appropriate default when this is `None`.
    #[serde(default, rename = "os-image-size-gib")]
    os_image_size_gib: Option<ImageSize>,
    #[serde(default = "ImageLayout::default_data_image_size_gib")]
    pub data_image_size_gib: ImageSize,
    #[serde(default = "ImageLayout::default_publish_image_size_hint_gib")]
    publish_image_size_hint_gib: ImageSize,
    #[serde(default = "ImageLayout::default_partition_plan")]
    pub partition_plan: PartitionPlan,
}

/// These are the historical defaults for all variants, before we added support
/// for customizing these properties.
static DEFAULT_OS_IMAGE_SIZE_GIB: ImageSize = ImageSize(2);
/// Default OS image size when `standalone-image = true` and the manifest
/// does not specify `os-image-size-gib`.
static DEFAULT_STANDALONE_IMAGE_OS_IMAGE_SIZE_GIB: ImageSize = ImageSize(1);
static DEFAULT_DATA_IMAGE_SIZE_GIB: ImageSize = ImageSize(1);
static DEFAULT_PUBLISH_IMAGE_SIZE_HINT_GIB: ImageSize = ImageSize(22);
static DEFAULT_PARTITION_PLAN: PartitionPlan = PartitionPlan::Split;

impl ImageLayout {
    fn default_data_image_size_gib() -> ImageSize {
        DEFAULT_DATA_IMAGE_SIZE_GIB
    }

    fn default_publish_image_size_hint_gib() -> ImageSize {
        DEFAULT_PUBLISH_IMAGE_SIZE_HINT_GIB
    }

    fn default_partition_plan() -> PartitionPlan {
        DEFAULT_PARTITION_PLAN
    }

    /// Returns the resolved OS image size. If the manifest did not specify
    /// `os-image-size-gib`, the historical 2 GiB default is used.
    pub fn os_image_size_gib(&self) -> ImageSize {
        self.os_image_size_gib.unwrap_or(DEFAULT_OS_IMAGE_SIZE_GIB)
    }

    /// Returns whether `os-image-size-gib` was explicitly set in the manifest.
    pub fn os_image_size_gib_was_set(&self) -> bool {
        self.os_image_size_gib.is_some()
    }

    // At publish time we will need specific sizes for the OS image and the (optional) data image.
    // The sizes returned by this function depend on the image layout, and whether the publish
    // image hint is larger than the required minimum size.
    pub fn publish_image_sizes_gib(&self) -> (i32, i32) {
        let os_image_base_size_gib = self.os_image_size_gib().0;
        let data_image_base_size_gib = self.data_image_size_gib.0;
        let publish_image_size_hint_gib = self.publish_image_size_hint_gib.0;

        let min_publish_image_size_gib = os_image_base_size_gib + data_image_base_size_gib;
        let publish_image_size_gib = max(publish_image_size_hint_gib, min_publish_image_size_gib);

        match self.partition_plan {
            PartitionPlan::Split => {
                let os_image_publish_size_gib = os_image_base_size_gib;
                let data_image_publish_size_gib = publish_image_size_gib - os_image_base_size_gib;
                (
                    os_image_publish_size_gib.into(),
                    data_image_publish_size_gib.into(),
                )
            }
            PartitionPlan::Unified => (publish_image_size_gib.into(), -1),
        }
    }
}

impl Default for ImageLayout {
    fn default() -> Self {
        Self {
            os_image_size_gib: None,
            data_image_size_gib: Self::default_data_image_size_gib(),
            publish_image_size_hint_gib: Self::default_publish_image_size_hint_gib(),
            partition_plan: Self::default_partition_plan(),
        }
    }
}

#[derive(Deserialize, Debug, Copy, Clone)]
#[serde(rename_all = "lowercase")]
pub enum PartitionPlan {
    Split,
    Unified,
}

#[derive(Deserialize, Serialize, Debug, Copy, Clone, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum SupportedArch {
    X86_64,
    Aarch64,
}

serde_plain::derive_fromstr_from_deserialize!(SupportedArch);
serde_plain::derive_display_from_serialize!(SupportedArch);

/// Map a Linux architecture into the corresponding Docker architecture.
impl SupportedArch {
    pub fn goarch(&self) -> &'static str {
        match self {
            SupportedArch::X86_64 => "amd64",
            SupportedArch::Aarch64 => "arm64",
        }
    }
}

#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(try_from = "String")]
pub enum ImageFeature {
    GrubSetPrivateVar,
    SystemdNetworkd,
    XfsDataPartition,
    ErofsRootPartition,
    UefiSecureBoot,
    Fips,
    InPlaceUpdates,
    HostContainers,
    ExternalKmodDevelopment,
    EncryptedStorage,
    StandaloneImage,
}

const EXPERIMENTAL_IMAGE_FEATURES: &[&ImageFeature] = &[&ImageFeature::EncryptedStorage];

const DEPRECATED_IMAGE_FEATURES: &[&ImageFeature] = &[
    &ImageFeature::GrubSetPrivateVar,
    &ImageFeature::SystemdNetworkd,
];

/// Returns an [`ImageLayout`] with fields adjusted for the given feature set
/// and image format.
///
/// - If `standalone-image` is enabled (or the image format is `eif`, which
///   is inherently a standalone-image image) and the manifest did not
///   explicitly set `os-image-size-gib`, the default OS image size is
///   reduced from 2 GiB to 1 GiB.
/// - If the image format is `eif`, the partition plan is forced to
///   `unified`. EIF images are inherently single-disk (no separate DATA
///   image), and `rpm2eif` ignores the partition-plan flag anyway; forcing
///   `unified` here means variants don't have to write out
///   `partition-plan = "unified"` boilerplate purely to satisfy the
///   validator, and the historical default (`split`) doesn't turn into an
///   error the moment the user opts into EIF.
///
/// `image_format` is optional so that call sites which don't yet have access
/// to the manifest's image format (or don't care) can pass `None` and get
/// the pre-existing `standalone-image`-only behavior.
pub fn resolved_image_layout(
    layout: &ImageLayout,
    features: &HashSet<ImageFeature>,
    image_format: Option<&ImageFormat>,
) -> ImageLayout {
    let mut resolved = *layout;
    let is_eif = matches!(image_format, Some(ImageFormat::Eif));
    let stripped = features.contains(&ImageFeature::StandaloneImage) || is_eif;
    if stripped && !layout.os_image_size_gib_was_set() {
        resolved.os_image_size_gib = Some(DEFAULT_STANDALONE_IMAGE_OS_IMAGE_SIZE_GIB);
    }
    if is_eif {
        resolved.partition_plan = PartitionPlan::Unified;
    }
    resolved
}

/// Validate that the requested image features and layout are compatible.
///
/// This enforces that enabling `standalone-image` is not combined with any
/// of `uefi-secure-boot`, `encrypted-storage`, `in-place-updates`,
/// `host-containers`, `xfs-data-partition`, or with a `split` partition plan.
///
/// An `image-format = "eif"` build is subject to the same constraints
/// regardless of the `standalone-image` toggle: an EIF is a single-bank,
/// dm-verity-protected, partitionless (no DATA/PRIVATE/RESERVED) image, and
/// `rpm2eif` silently ignores every one of those knobs. Enforcing the
/// constraints up-front turns a subtle "build succeeds, image is wrong"
/// footgun into a clear build-time error.
///
/// All discovered conflicts are reported in a single error so that users see
/// the complete list in one build cycle. The conflict list is iterated in a
/// fixed order, so the resulting error message is deterministic.
///
/// # Internal contract
///
/// Callers that pass `features` from [`ManifestInfo::image_features`] and
/// `layout` from [`resolved_image_layout`] should invoke this function before
/// constructing any `DockerBuild` or otherwise using those values to drive a
/// build. Both `DockerBuild::new_variant` and `DockerBuild::repack_variant`
/// call this internally as defense-in-depth so that adding a new entry point
/// cannot silently bypass validation.
pub fn validate_image_features(
    features: &HashSet<ImageFeature>,
    layout: &ImageLayout,
    image_format: Option<&ImageFormat>,
) -> Result<()> {
    let is_eif = matches!(image_format, Some(ImageFormat::Eif));
    // Without `standalone-image` enabled *and* a non-EIF format, every
    // combination is legal (full Bottlerocket image).
    if !features.contains(&ImageFeature::StandaloneImage) && !is_eif {
        return Ok(());
    }

    // Explain conflicts as either a standalone-image violation or an EIF
    // violation, whichever applies. EIF takes precedence in the message
    // because it's the more surprising case (the format silently strips
    // features rather than an explicit `standalone-image = true`).
    let context: &str = if is_eif {
        "`image-format = \"eif\"`"
    } else {
        "`standalone-image = true`"
    };
    let reason_secure_boot: String = if is_eif {
        // Note: EIF *signing* (a COSE_Sign1 over PCR0 embedded in an
        // `EifSectionSignature`) is orthogonal to UEFI Secure Boot. The
        // former is enabled automatically by `rpm2eif` when Infra.toml
        // carries an `[eif]` section (backed by either a local PEM key
        // or a KMS key id). The latter -- signing shim/grub/vmlinuz as
        // PE Authenticode -- has no place in the EIF pipeline and is
        // still rejected here.
        "secure boot is not supported by the EIF pipeline; `rpm2eif` does not sign shim/grub/vmlinuz (EIF signing over PCR0 is orthogonal and driven by Infra.toml [eif])".to_string()
    } else {
        "secure boot relies on signed first-party artifacts that are not present when `standalone-image = true`".to_string()
    };
    let reason_encrypted_storage: String = if is_eif {
        "encrypted storage requires the BOTTLEROCKET-PRIVATE/DATA partitions, which an EIF image does not have".to_string()
    } else {
        "encrypted storage requires the BOTTLEROCKET-PRIVATE/DATA partitions, which are not built when `standalone-image = true`".to_string()
    };
    let reason_ipu: String = if is_eif {
        "in-place updates require two banks of OS partitions; an EIF ships a single ROOT-A/HASH-A pair".to_string()
    } else {
        "in-place updates require two banks of OS partitions, which are not built when `standalone-image = true`".to_string()
    };
    let reason_host_containers: String = if is_eif {
        "host-containers require the BOTTLEROCKET-DATA partition, which an EIF image does not have"
            .to_string()
    } else {
        "host-containers require the BOTTLEROCKET-DATA partition, which is not built when `standalone-image = true`".to_string()
    };
    let reason_standalone_image_missing =
        "the EIF pipeline is inherently stripped down; `rpm2eif` requires `standalone-image = true`";
    let reason_xfs_data_partition: String = if is_eif {
        "xfs-data-partition requires the BOTTLEROCKET-DATA partition, which an EIF image does not have".to_string()
    } else {
        "xfs-data-partition requires the BOTTLEROCKET-DATA partition, which is not built when `standalone-image = true`".to_string()
    };

    let conflicts: &[(ImageFeature, &str, &str)] = &[
        (
            ImageFeature::XfsDataPartition,
            "xfs-data-partition",
            &reason_xfs_data_partition,
        ),
        (
            ImageFeature::UefiSecureBoot,
            "uefi-secure-boot",
            &reason_secure_boot,
        ),
        (
            ImageFeature::EncryptedStorage,
            "encrypted-storage",
            &reason_encrypted_storage,
        ),
        (
            ImageFeature::InPlaceUpdates,
            "in-place-updates",
            &reason_ipu,
        ),
        (
            ImageFeature::HostContainers,
            "host-containers",
            &reason_host_containers,
        ),
    ];

    // Collect all conflicts in a deterministic order so the user sees the
    // complete list in a single build cycle, rather than playing whack-a-mole.
    // Use the kebab-case manifest key in messages to match the TOML the user
    // wrote and the error messages produced by the shell-side validators.
    let mut conflict_messages: Vec<String> = conflicts
        .iter()
        .filter(|(feature, _, _)| features.contains(feature))
        .map(|(_, name, reason)| format!("`{name}`: {reason}"))
        .collect();

    // EIF requires standalone-image to be present in the feature set.
    if is_eif && !features.contains(&ImageFeature::StandaloneImage) {
        conflict_messages.insert(
            0,
            format!("`standalone-image`: {reason_standalone_image_missing}"),
        );
    }

    if matches!(layout.partition_plan, PartitionPlan::Split) {
        let reason = if is_eif {
            "`partition-plan=split`: an EIF is a single unified disk; the split \
             layout has no meaning"
        } else {
            "`partition-plan=split`: when `standalone-image = true`, the image uses a \
             single unified disk with no separate data image"
        };
        conflict_messages.push(reason.to_string());
    }

    if !conflict_messages.is_empty() {
        // Render one conflict per line, indented, so the resulting error
        // message remains readable when several conflicts are reported at
        // once.
        let reason = format!("\n  - {}", conflict_messages.join("\n  - "));
        return error::IncompatibleImageFeaturesSnafu { context, reason }.fail()?;
    }

    Ok(())
}

impl TryFrom<String> for ImageFeature {
    type Error = Error;
    fn try_from(s: String) -> Result<Self> {
        match s.as_str() {
            "grub-set-private-var" => Ok(ImageFeature::GrubSetPrivateVar),
            "systemd-networkd" => Ok(ImageFeature::SystemdNetworkd),
            "xfs-data-partition" => Ok(ImageFeature::XfsDataPartition),
            "erofs-root-partition" => Ok(ImageFeature::ErofsRootPartition),
            "uefi-secure-boot" => Ok(ImageFeature::UefiSecureBoot),
            "fips" => Ok(ImageFeature::Fips),
            "in-place-updates" => Ok(ImageFeature::InPlaceUpdates),
            "host-containers" => Ok(ImageFeature::HostContainers),
            "external-kmod-development" => Ok(ImageFeature::ExternalKmodDevelopment),
            "encrypted-storage" => Ok(ImageFeature::EncryptedStorage),
            "standalone-image" => Ok(ImageFeature::StandaloneImage),
            _ => error::ParseImageFeatureSnafu { what: s }.fail()?,
        }
    }
}

/// String representations of image-feature flags across the toolchain.
///
/// A single feature has multiple string representations as it flows through
/// the build pipeline. Keep these in mind when adding or modifying features:
///
/// | Layer                              | Representation       | Example          |
/// |------------------------------------|----------------------|------------------|
/// | Rust enum variant                  | UpperCamelCase       | `StandaloneImage` |
/// | Manifest TOML key (TryFrom impl)   | kebab-case           | `standalone-image` |
/// | Display impl (env var name)        | UPPER_SNAKE_CASE     | `STANDALONE_IMAGE` |
/// | Dockerfile `--build-arg` value     | `1` (truthy) or `yes` | `STANDALONE_IMAGE=yes` |
/// | Shell script `--with-X=` value     | `yes` / `no`         | `--with-standalone-image=yes` |
/// | Runtime `image-features.env` value | `true` / `false`     | `STANDALONE_IMAGE=true` |
///
/// Each layer's representation is intentional: the Dockerfile uses `1` so
/// that bash's `${VAR:+...}` expansion can convert presence to a flag, the
/// shell scripts use `yes`/`no` to be readable in build logs, and the
/// runtime env file uses `true`/`false` to be parseable by configuration
/// loaders. `StandaloneImage` is the exception: since it is default-false
/// and opt-in, its build-arg is `yes` so the Dockerfile can pipe the
/// value directly to `--with-standalone-image=`. See
/// `tools/buildsys/src/builder.rs`, `twoliter/embedded/build.Dockerfile`,
/// and `twoliter/embedded/rpm2img` for the conversions between layers.
impl fmt::Display for ImageFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageFeature::GrubSetPrivateVar => write!(f, "GRUB_SET_PRIVATE_VAR"),
            ImageFeature::SystemdNetworkd => write!(f, "SYSTEMD_NETWORKD"),
            ImageFeature::XfsDataPartition => write!(f, "XFS_DATA_PARTITION"),
            ImageFeature::ErofsRootPartition => write!(f, "EROFS_ROOT_PARTITION"),
            ImageFeature::UefiSecureBoot => write!(f, "UEFI_SECURE_BOOT"),
            ImageFeature::Fips => write!(f, "FIPS"),
            ImageFeature::InPlaceUpdates => write!(f, "IN_PLACE_UPDATES"),
            ImageFeature::HostContainers => write!(f, "HOST_CONTAINERS"),
            ImageFeature::ExternalKmodDevelopment => write!(f, "EXTERNAL_KMOD_DEVELOPMENT"),
            ImageFeature::EncryptedStorage => write!(f, "ENCRYPTED_STORAGE"),
            ImageFeature::StandaloneImage => write!(f, "STANDALONE_IMAGE"),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum BundleModule {
    Go,
    Rust,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct ExternalFile {
    pub path: Option<PathBuf>,
    pub sha512: String,
    pub url: String,
    pub force_upstream: Option<bool>,
    pub bundle_modules: Option<Vec<BundleModule>>,
    pub bundle_root_path: Option<PathBuf>,
    pub bundle_output_path: Option<PathBuf>,
}

// =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=

#[cfg(test)]
mod test {
    use super::*;
    use guppy::MetadataCommand;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn test_projects_dir() -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.pop();
        p.join("tests").join("projects")
    }

    // Fixture manifest paths must be canonicalized so they match the paths that `cargo
    // metadata` emits (macOS resolves `/var` -> `/private/var`, etc.); `Manifest::new`
    // looks up the graph node by exact path. Symlink resolution is the desired behavior
    // here, so opt out of the workspace-wide `canonicalize` lint at each helper.
    #[allow(clippy::disallowed_methods)]
    fn cargo_manifest(name: &str) -> PathBuf {
        let subdir = if name.starts_with("pkg-") {
            "packages"
        } else if name.ends_with("kit") {
            "kits"
        } else {
            "variants"
        };

        let path = test_projects_dir()
            .join("local-kit")
            .join(subdir)
            .join(name)
            .join("Cargo.toml");
        path.canonicalize()
            .unwrap_or_else(|_| panic!("unable to canonicalize {}", path.display()))
    }

    fn cargo_metadata_path(temp_dir: &TempDir) -> PathBuf {
        let output_path = temp_dir.path().join("cargo_metadata.json");
        let output = MetadataCommand::new()
            .manifest_path(test_projects_dir().join("local-kit").join("Cargo.toml"))
            .current_dir(temp_dir.path())
            .other_options(["--locked", "--frozen", "--offline"])
            .cargo_command()
            .output()
            .unwrap();

        if !output.status.success() {
            panic!("cargo command failed {:?}", output)
        }

        fs::write(&output_path, output.stdout).unwrap();
        output_path
    }

    #[test]
    fn test_package_list_pkg_g() {
        let manifest_path = cargo_manifest("pkg-g");
        let temp_dir = TempDir::new().unwrap();
        let cargo_metadata_path = cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, cargo_metadata_path).unwrap();
        let package_list = manifest.package_dependencies().unwrap();
        assert!(package_list.is_empty());
    }

    /// This test confirms that we are using the `build-package.package-name` if there is one when
    /// returning lists from the Cargo graph.
    #[test]
    fn test_package_list_core_kit() {
        let manifest_path = cargo_manifest("core-kit");
        let temp_dir = TempDir::new().unwrap();
        let cargo_metadata_path = cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, cargo_metadata_path).unwrap();
        let package_list = manifest.package_dependencies().unwrap();
        let expected = vec!["pkg-a-1.27".to_string()];
        assert_eq!(package_list, expected);
    }

    #[test]
    fn test_package_list_extra_3_kit() {
        let manifest_path = cargo_manifest("extra-3-kit");
        let temp_dir = TempDir::new().unwrap();
        let cargo_metadata_path = cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, cargo_metadata_path).unwrap();
        let package_list = manifest.package_dependencies().unwrap();
        let expected = vec![
            "pkg-e".to_string(),
            "pkg-f".to_string(),
            "pkg-g".to_string(),
        ];
        assert_eq!(package_list, expected);
    }

    #[test]
    fn test_kit_dependencies_pkg_e() {
        let manifest_path = cargo_manifest("pkg-e");
        let temp_dir = TempDir::new().unwrap();
        let cargo_metadata_path = cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, cargo_metadata_path).unwrap();
        let kit_list = manifest.kit_dependencies().unwrap();
        let expected = vec![
            "core-kit".to_string(),
            "extra-1-kit".to_string(),
            "extra-2-kit".to_string(),
        ];
        assert_eq!(kit_list, expected);
    }

    #[test]
    fn test_kit_dependencies_variant_hello_ootb() {
        let manifest_path = cargo_manifest("hello-ootb");
        let temp_dir = TempDir::new().unwrap();
        let cargo_metadata_path = cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, cargo_metadata_path).unwrap();
        let kit_list = manifest.kit_dependencies().unwrap();
        let expected = vec![
            "core-kit".to_string(),
            "extra-1-kit".to_string(),
            "extra-2-kit".to_string(),
            "extra-3-kit".to_string(),
        ];
        assert_eq!(kit_list, expected);
    }

    fn standalone_image_layout(os_size: Option<u16>, plan: PartitionPlan) -> ImageLayout {
        ImageLayout {
            os_image_size_gib: os_size.map(ImageSize),
            data_image_size_gib: DEFAULT_DATA_IMAGE_SIZE_GIB,
            publish_image_size_hint_gib: DEFAULT_PUBLISH_IMAGE_SIZE_HINT_GIB,
            partition_plan: plan,
        }
    }

    #[test]
    fn standalone_image_enabled_default_os_size_is_one_gib() {
        let layout = standalone_image_layout(None, PartitionPlan::Unified);
        // StandaloneImage in the set → stripped image, default 1 GiB.
        let features = HashSet::from([ImageFeature::StandaloneImage]);
        let resolved = resolved_image_layout(&layout, &features, None);
        assert_eq!(resolved.os_image_size_gib().0, 1);
    }

    #[test]
    fn standalone_image_enabled_respects_explicit_os_size() {
        let layout = standalone_image_layout(Some(4), PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::StandaloneImage]);
        let resolved = resolved_image_layout(&layout, &features, None);
        assert_eq!(resolved.os_image_size_gib().0, 4);
    }

    #[test]
    fn standalone_image_absent_default_os_size_is_two_gib() {
        let layout = standalone_image_layout(None, PartitionPlan::Unified);
        let features: HashSet<ImageFeature> = HashSet::new();
        let resolved = resolved_image_layout(&layout, &features, None);
        assert_eq!(resolved.os_image_size_gib().0, 2);
    }

    #[test]
    fn standalone_image_enabled_alone_passes_validation() {
        let layout = standalone_image_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::StandaloneImage]);
        validate_image_features(&features, &layout, None)
            .expect("standalone-image=true + unified should validate");
    }

    #[test]
    fn standalone_image_enabled_rejects_secure_boot() {
        let layout = standalone_image_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::StandaloneImage, ImageFeature::UefiSecureBoot]);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn standalone_image_enabled_rejects_encrypted_storage() {
        let layout = standalone_image_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([
            ImageFeature::StandaloneImage,
            ImageFeature::EncryptedStorage,
        ]);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn standalone_image_enabled_rejects_in_place_updates() {
        let layout = standalone_image_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::StandaloneImage, ImageFeature::InPlaceUpdates]);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn standalone_image_enabled_rejects_split_partition_plan() {
        let layout = standalone_image_layout(None, PartitionPlan::Split);
        let features = HashSet::from([ImageFeature::StandaloneImage]);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn validation_is_noop_when_standalone_image_absent() {
        // Without `standalone-image`, the validator should accept any
        // combination (full image).
        let layout = standalone_image_layout(None, PartitionPlan::Split);
        let features = HashSet::from([
            ImageFeature::UefiSecureBoot,
            ImageFeature::EncryptedStorage,
            ImageFeature::InPlaceUpdates,
        ]);
        validate_image_features(&features, &layout, None)
            .expect("validation should be no-op without standalone-image");
    }

    #[test]
    fn standalone_image_enabled_rejects_host_containers() {
        let layout = standalone_image_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::StandaloneImage, ImageFeature::HostContainers]);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn standalone_image_enabled_rejects_xfs_data_partition() {
        // `standalone-image = true` skips building the BOTTLEROCKET-DATA
        // partition, so opting into `xfs-data-partition` on top of it would
        // silently do nothing. Reject the combination at build time.
        let layout = standalone_image_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([
            ImageFeature::StandaloneImage,
            ImageFeature::XfsDataPartition,
        ]);
        let err = validate_image_features(&features, &layout, None)
            .expect_err("standalone-image=true + xfs-data-partition must fail validation");
        let msg = format!("{err}");
        assert!(
            msg.contains("xfs-data-partition"),
            "missing 'xfs-data-partition' in: {msg}",
        );
        assert!(
            msg.contains("standalone-image = true"),
            "message should reference standalone-image=true: {msg}",
        );
    }

    #[test]
    fn standalone_image_enabled_reports_all_conflicts_at_once() {
        // The validator should report every conflict in a single error so the
        // user does not have to fix them one at a time across multiple builds.
        let layout = standalone_image_layout(None, PartitionPlan::Split);
        let features = HashSet::from([
            ImageFeature::StandaloneImage,
            ImageFeature::UefiSecureBoot,
            ImageFeature::EncryptedStorage,
            ImageFeature::InPlaceUpdates,
            ImageFeature::HostContainers,
            ImageFeature::XfsDataPartition,
        ]);
        let err = validate_image_features(&features, &layout, None)
            .expect_err("multi-conflict combination should fail validation");
        let msg = format!("{err}");
        assert!(
            msg.contains("uefi-secure-boot"),
            "missing uefi-secure-boot in: {msg}"
        );
        assert!(
            msg.contains("encrypted-storage"),
            "missing encrypted-storage in: {msg}"
        );
        assert!(
            msg.contains("in-place-updates"),
            "missing in-place-updates in: {msg}"
        );
        assert!(
            msg.contains("host-containers"),
            "missing host-containers in: {msg}"
        );
        assert!(
            msg.contains("xfs-data-partition"),
            "missing xfs-data-partition in: {msg}"
        );
        assert!(
            msg.contains("partition-plan=split"),
            "missing partition-plan in: {msg}"
        );
    }

    /// Build a `ManifestInfo` from a TOML manifest fragment string.
    fn manifest_info_from_toml(toml_str: &str) -> ManifestInfo {
        toml::from_str(toml_str).expect("failed to parse test manifest")
    }

    /// Build a `[package.metadata.build-variant]` manifest with the given
    /// inline `image-features` map.
    fn variant_manifest_with_image_features(image_features_block: &str) -> String {
        format!(
            r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
{image_features_block}
"#
        )
    }

    #[test]
    fn image_features_standalone_image_enabled_drops_silent_defaults() {
        // `standalone-image = true` alone should drop the silent
        // `in-place-updates` and `host-containers` defaults and add
        // `standalone-image` to the seed.
        let toml = variant_manifest_with_image_features(
            "[package.metadata.build-variant.image-features]\nstandalone-image = true",
        );
        let info = manifest_info_from_toml(&toml);
        let features = info.image_features().expect("variant has image-features");
        assert!(features.contains(&ImageFeature::StandaloneImage));
        assert!(features.contains(&ImageFeature::ExternalKmodDevelopment));
        assert!(!features.contains(&ImageFeature::InPlaceUpdates));
        assert!(!features.contains(&ImageFeature::HostContainers));
    }

    #[test]
    fn image_features_standalone_image_enabled_with_host_containers_keeps_both() {
        // The seed-set logic adds host-containers back if it's explicitly set
        // to `true`, so that the validator can later reject the combination
        // with a clear error rather than silently producing a confused image.
        let toml = variant_manifest_with_image_features(
            "[package.metadata.build-variant.image-features]\n\
             standalone-image = true\n\
             host-containers = true",
        );
        let info = manifest_info_from_toml(&toml);
        let features = info.image_features().expect("variant has image-features");
        assert!(features.contains(&ImageFeature::StandaloneImage));
        assert!(features.contains(&ImageFeature::HostContainers));
        // Validator should reject this combination.
        let layout = standalone_image_layout(None, PartitionPlan::Unified);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn image_features_default_seed_does_not_include_standalone_image() {
        // No `image-features` at all → the default seed does NOT include
        // `standalone-image` (full image) and includes the historical
        // silent defaults.
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
"#;
        let info = manifest_info_from_toml(toml);
        let features = info
            .image_features()
            .expect("variant has build-variant section");
        assert!(!features.contains(&ImageFeature::StandaloneImage));
        assert!(features.contains(&ImageFeature::InPlaceUpdates));
        assert!(features.contains(&ImageFeature::HostContainers));
        assert!(features.contains(&ImageFeature::ExternalKmodDevelopment));
    }

    #[test]
    fn image_features_explicit_standalone_image_false_preserves_defaults() {
        // Explicit `standalone-image = false` is a no-op vs. the default; the
        // historical silent defaults remain.
        let toml = variant_manifest_with_image_features(
            "[package.metadata.build-variant.image-features]\nstandalone-image = false",
        );
        let info = manifest_info_from_toml(&toml);
        let features = info.image_features().expect("variant has image-features");
        assert!(!features.contains(&ImageFeature::StandaloneImage));
        assert!(features.contains(&ImageFeature::InPlaceUpdates));
        assert!(features.contains(&ImageFeature::HostContainers));
        assert!(features.contains(&ImageFeature::ExternalKmodDevelopment));
    }

    #[test]
    fn eif_pcie_flags_omitted_is_none() {
        // A variant without `eif-pcie-flags` returns None, so buildsys emits
        // the empty ARG and rpm2eif/eif2eif omit `--pcie-flags`, letting
        // `eif-builder` apply its `DEFAULT_PCIE_FLAGS`.
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
"#;
        let info = manifest_info_from_toml(toml);
        assert_eq!(info.eif_pcie_flags(), None);
    }

    #[test]
    fn eif_pcie_flags_hex_string_parses() {
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
eif-pcie-flags = "0x340"
"#;
        let info = manifest_info_from_toml(toml);
        assert_eq!(info.eif_pcie_flags(), Some(0x340));
    }

    #[test]
    fn eif_pcie_flags_unprefixed_string_is_rejected() {
        // `"340"` is ambiguous (hex-in-the-CLI-sense vs decimal-in-Rust-sense)
        // and almost certainly a copy-paste mistake, so we reject it at
        // parse time with a clear message. The author must write `"0x340"`
        // for hex or the bare integer `832` for decimal.
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
eif-pcie-flags = "340"
"#;
        let err = toml::from_str::<ManifestInfo>(toml)
            .expect_err("unprefixed hex string should be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("0x"),
            "error should mention the `0x` prefix requirement, got: {msg}"
        );
    }

    #[test]
    fn eif_pcie_flags_uppercase_prefix_parses() {
        // `0X` prefix is accepted alongside `0x`, matching eif-builder's own
        // `--pcie-flags` parser.
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
eif-pcie-flags = "0X340"
"#;
        let info = manifest_info_from_toml(toml);
        assert_eq!(info.eif_pcie_flags(), Some(0x340));
    }

    #[test]
    fn eif_pcie_flags_integer_parses() {
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
eif-pcie-flags = 576
"#;
        let info = manifest_info_from_toml(toml);
        assert_eq!(info.eif_pcie_flags(), Some(576));
    }

    #[test]
    fn eif_pcie_flags_rejects_overflow() {
        // 0x10000 does not fit in u16; the deserializer surfaces a clear error
        // rather than silently truncating.
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
eif-pcie-flags = "0x10000"
"#;
        let err = toml::from_str::<ManifestInfo>(toml).expect_err("0x10000 should not fit in u16");
        let msg = format!("{err}");
        assert!(
            msg.to_lowercase().contains("u16"),
            "expected u16 mention in error, got: {msg}"
        );
    }

    #[test]
    fn eif_kernel_format_omitted_is_none() {
        // A variant without `eif-kernel-format` returns None, so buildsys
        // emits the empty ARG and rpm2eif applies its built-in default
        // (bzImage on x86_64 -- the sidecar Nitro Enclaves loader boot
        // protocol).
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
"#;
        let info = manifest_info_from_toml(toml);
        assert_eq!(info.eif_kernel_format(), None);
    }

    #[test]
    fn eif_kernel_format_bzimage_parses() {
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
eif-kernel-format = "bzimage"
"#;
        let info = manifest_info_from_toml(toml);
        assert_eq!(info.eif_kernel_format(), Some(EifKernelFormat::Bzimage));
        assert_eq!(info.eif_kernel_format().unwrap().as_str(), "bzimage");
    }

    #[test]
    fn eif_kernel_format_vmlinux_parses() {
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
eif-kernel-format = "vmlinux"
"#;
        let info = manifest_info_from_toml(toml);
        assert_eq!(info.eif_kernel_format(), Some(EifKernelFormat::Vmlinux));
        assert_eq!(info.eif_kernel_format().unwrap().as_str(), "vmlinux");
    }

    #[test]
    fn eif_kernel_format_unknown_is_rejected() {
        // An unrecognized value must fail at parse time rather than falling
        // silently through to the default; the rpm2eif side also validates
        // the flag, but catching typos here gives variant authors a
        // per-manifest error instead of a container-build failure.
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
eif-kernel-format = "elf"
"#;
        let err = toml::from_str::<ManifestInfo>(toml)
            .expect_err("unknown eif-kernel-format value should be rejected");
        let msg = format!("{err}");
        assert!(
            msg.to_lowercase().contains("elf")
                || msg.to_lowercase().contains("variant")
                || msg.to_lowercase().contains("unknown"),
            "error should mention the bad value, got: {msg}"
        );
    }

    #[test]
    fn image_features_omitted_uses_default_image() {
        // A variant that omits the `[image-features]` section entirely
        // produces the standard Bottlerocket image: `standalone-image` is NOT
        // in the silent default seed, and the resolved layout uses the
        // historical 2 GiB OS size.
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
"#;
        let info = manifest_info_from_toml(toml);
        let features = info
            .image_features()
            .expect("variant has build-variant section");
        let raw_layout = ImageLayout::default();
        let layout = resolved_image_layout(&raw_layout, &features, None);
        assert_eq!(layout.os_image_size_gib().0, 2);
        validate_image_features(&features, &layout, None)
            .expect("default image should pass validation");
    }

    /// Verify that a variant declaring `image-format = "eif"` parses and
    /// surfaces `ImageFormat::Eif` from `image_format()`.
    #[test]
    fn test_image_format_eif_variant_hello_eif() {
        let manifest_path = cargo_manifest("hello-eif");
        let temp_dir = TempDir::new().unwrap();
        let cargo_metadata_path = cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, cargo_metadata_path).unwrap();
        assert!(matches!(
            manifest.info().image_format(),
            Some(ImageFormat::Eif)
        ));
    }

    /// Sanity check: the existing non-EIF variant has no `image-format` set and
    /// therefore returns `None`. This guards the default-`raw` path used by
    /// `builder.rs`.
    #[test]
    fn test_image_format_default_variant_hello_ootb() {
        let manifest_path = cargo_manifest("hello-ootb");
        let temp_dir = TempDir::new().unwrap();
        let cargo_metadata_path = cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, cargo_metadata_path).unwrap();
        assert!(manifest.info().image_format().is_none());
    }

    // ---------------------------------------------------------------------
    // `image-format = "eif"` validation and layout-resolution tests.
    //
    // These mirror the `standalone_image_enabled_*` tests but with the EIF
    // image format as the trigger instead of an explicit
    // `standalone-image = true`. EIF is inherently a stripped-down,
    // single-bank image; the validator must reject conflicting features so
    // that misconfiguration surfaces at build time rather than silently
    // producing a broken artifact via `rpm2eif`'s reduced pipeline.
    // ---------------------------------------------------------------------

    #[test]
    fn eif_format_default_os_size_is_one_gib() {
        // Same rationale as `standalone_image_enabled_default_os_size_is_one_gib`:
        // a stripped-down layout doesn't need 2 GiB of slack.
        let layout = ImageLayout::default();
        let features = HashSet::from([ImageFeature::ExternalKmodDevelopment]);
        let resolved = resolved_image_layout(&layout, &features, Some(&ImageFormat::Eif));
        assert_eq!(resolved.os_image_size_gib().0, 1);
    }

    #[test]
    fn eif_format_respects_explicit_os_size() {
        // If a variant explicitly sets `os-image-size-gib`, the EIF path
        // must honor it (rpm2eif will pad the disk image to that size).
        let layout = ImageLayout {
            os_image_size_gib: Some(ImageSize(3)),
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::ExternalKmodDevelopment]);
        let resolved = resolved_image_layout(&layout, &features, Some(&ImageFormat::Eif));
        assert_eq!(resolved.os_image_size_gib().0, 3);
    }

    #[test]
    fn eif_format_alone_passes_validation() {
        // The minimal EIF feature set (StandaloneImage +
        // ExternalKmodDevelopment) with `partition-plan = unified` should
        // validate successfully.
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([
            ImageFeature::StandaloneImage,
            ImageFeature::ExternalKmodDevelopment,
        ]);
        validate_image_features(&features, &layout, Some(&ImageFormat::Eif))
            .expect("EIF + minimal features + unified should validate");
    }

    #[test]
    fn eif_format_rejects_secure_boot() {
        // `uefi-secure-boot` is meaningless in the EIF pipeline: `rpm2eif`
        // does not sign shim/grub/vmlinuz.
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::UefiSecureBoot]);
        let err = validate_image_features(&features, &layout, Some(&ImageFormat::Eif))
            .expect_err("EIF + uefi-secure-boot must fail validation");
        let msg = format!("{err}");
        assert!(
            msg.contains("image-format = \"eif\""),
            "wrong prefix: {msg}"
        );
        assert!(
            msg.contains("uefi-secure-boot"),
            "missing feature name: {msg}"
        );
    }

    #[test]
    fn eif_format_rejects_encrypted_storage() {
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::EncryptedStorage]);
        assert!(validate_image_features(&features, &layout, Some(&ImageFormat::Eif)).is_err());
    }

    #[test]
    fn eif_format_rejects_in_place_updates() {
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::InPlaceUpdates]);
        assert!(validate_image_features(&features, &layout, Some(&ImageFormat::Eif)).is_err());
    }

    #[test]
    fn eif_format_rejects_host_containers() {
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::HostContainers]);
        assert!(validate_image_features(&features, &layout, Some(&ImageFormat::Eif)).is_err());
    }

    #[test]
    fn eif_format_rejects_missing_standalone_image() {
        // EIF requires standalone-image to be present (stripped image).
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features: HashSet<ImageFeature> = HashSet::new();
        let err = validate_image_features(&features, &layout, Some(&ImageFormat::Eif))
            .expect_err("EIF without standalone-image must fail validation");
        assert!(format!("{err}").contains("standalone-image"));
    }

    #[test]
    fn eif_format_rejects_xfs_data_partition() {
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::XfsDataPartition]);
        let err = validate_image_features(&features, &layout, Some(&ImageFormat::Eif))
            .expect_err("EIF + xfs-data-partition must fail validation");
        assert!(format!("{err}").contains("xfs-data-partition"));
    }

    #[test]
    fn eif_format_rejects_split_partition_plan() {
        // EIF images are inherently unified single-disk images; the split
        // plan has no meaning.
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Split,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::ExternalKmodDevelopment]);
        assert!(validate_image_features(&features, &layout, Some(&ImageFormat::Eif)).is_err());
    }

    #[test]
    fn eif_format_rejects_in_place_updates_even_without_standalone_image() {
        // Even if the user does not explicitly enable `standalone-image`,
        // `image-format = "eif"` must still reject the conflicting features.
        // Without this the EIF path could silently ignore, e.g.,
        // `in-place-updates = true` and produce an image the author did not
        // ask for.
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::InPlaceUpdates]);
        assert!(
            validate_image_features(&features, &layout, Some(&ImageFormat::Eif)).is_err(),
            "EIF must reject conflicts even when standalone-image is not explicitly set"
        );
    }

    #[test]
    fn eif_format_reports_all_conflicts_at_once() {
        // Same "don't play whack-a-mole" guarantee as the standalone-image
        // validator: reject everything in one build cycle.
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Split,
            ..ImageLayout::default()
        };
        let features = HashSet::from([
            ImageFeature::XfsDataPartition,
            ImageFeature::UefiSecureBoot,
            ImageFeature::EncryptedStorage,
            ImageFeature::InPlaceUpdates,
            ImageFeature::HostContainers,
        ]);
        let err = validate_image_features(&features, &layout, Some(&ImageFormat::Eif))
            .expect_err("multi-conflict EIF should fail validation");
        let msg = format!("{err}");
        for keyword in [
            "standalone-image",
            "xfs-data-partition",
            "uefi-secure-boot",
            "encrypted-storage",
            "in-place-updates",
            "host-containers",
            "partition-plan=split",
        ] {
            assert!(msg.contains(keyword), "missing '{keyword}' in: {msg}");
        }
    }

    #[test]
    fn eif_format_seeds_stripped_defaults() {
        // A variant with `image-format = "eif"` and no `[image-features]`
        // section must get `standalone-image` seeded and NOT get the silent
        // IPU / host-containers defaults. Otherwise the EIF validator would
        // immediately reject an otherwise-valid manifest.
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
image-format = "eif"
"#;
        let info = manifest_info_from_toml(toml);
        let features = info
            .image_features()
            .expect("variant has build-variant section");
        assert!(features.contains(&ImageFeature::StandaloneImage));
        assert!(!features.contains(&ImageFeature::InPlaceUpdates));
        assert!(!features.contains(&ImageFeature::HostContainers));
        assert!(features.contains(&ImageFeature::ExternalKmodDevelopment));
        // And the resolved layout picks up the 1 GiB EIF default *and* the
        // unified partition plan (overriding the historical `split` default).
        let raw_layout = ImageLayout::default();
        let layout = resolved_image_layout(&raw_layout, &features, Some(&ImageFormat::Eif));
        assert_eq!(layout.os_image_size_gib().0, 1);
        assert!(matches!(layout.partition_plan, PartitionPlan::Unified));
        validate_image_features(&features, &layout, Some(&ImageFormat::Eif))
            .expect("default EIF variant should pass validation");
    }

    #[test]
    fn eif_format_forces_unified_partition_plan() {
        // Even if a user explicitly writes `partition-plan = "split"`, the
        // resolver must override it. The alternative — letting the split
        // survive and having the validator reject it — is worse UX: the
        // user gets a build failure for a knob that `rpm2eif` was going to
        // ignore anyway.
        let raw_layout = ImageLayout {
            partition_plan: PartitionPlan::Split,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::ExternalKmodDevelopment]);
        let resolved = resolved_image_layout(&raw_layout, &features, Some(&ImageFormat::Eif));
        assert!(matches!(resolved.partition_plan, PartitionPlan::Unified));
    }

    /// `build_type()` returns `BuildType::Variant` and `guest_images()` round-trips when the
    /// manifest carries a `[package.metadata.build-variant]` section with `guest-images`.
    #[test]
    fn test_build_variant_with_guest_images() {
        let toml = r#"
[package]
name = "wrapper-variant"
version = "0.1.0"

[package.metadata.build-variant]
guest-images = { "inner-variant" = "/usr/share/bottlerocket/guests/inner" }
"#;
        let info: ManifestInfo = toml::from_str(toml).unwrap();
        assert_eq!(info.build_type().unwrap(), BuildType::Variant);
        let guests = info.guest_images().expect("guest-images should round-trip");
        assert_eq!(guests.len(), 1);
        assert_eq!(
            guests.get("inner-variant").map(|p| p.as_path()),
            Some(std::path::Path::new("/usr/share/bottlerocket/guests/inner"))
        );
    }

    /// A variant manifest without `guest-images` returns `None` from the accessor.
    #[test]
    fn test_build_variant_without_guest_images() {
        let toml = r#"
[package]
name = "plain-variant"
version = "0.1.0"

[package.metadata.build-variant]
"#;
        let info: ManifestInfo = toml::from_str(toml).unwrap();
        assert_eq!(info.build_type().unwrap(), BuildType::Variant);
        assert!(info.guest_images().is_none());
    }

    /// A manifest without any build-* metadata still defaults to `Package` (preserved behavior).
    #[test]
    fn test_build_type_default_package_unchanged() {
        let toml = r#"
[package]
name = "ordinary"
version = "0.1.0"
"#;
        let info: ManifestInfo = toml::from_str(toml).unwrap();
        assert_eq!(info.build_type().unwrap(), BuildType::Package);
    }

    /// Helper: build a `BTreeMap` for `validate_guest_image_entries` from a slice of pairs.
    fn guest_map(entries: &[(&str, &str)]) -> BTreeMap<String, PathBuf> {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_string(), PathBuf::from(*v)))
            .collect()
    }

    /// Empty / valid input is accepted.
    #[test]
    fn test_validate_guest_image_entries_accepts_well_formed_input() {
        let map = guest_map(&[
            ("inner-variant", "/usr/share/bottlerocket/guests/inner"),
            ("other-guest", "/var/lib/guests/other"),
        ]);
        validate_guest_image_entries("host-variant", &map, None)
            .expect("well-formed input must pass");
    }

    /// A variant cannot use itself as a guest. The error must mention the variant name.
    #[test]
    fn test_validate_guest_image_entries_rejects_self_reference() {
        let map = guest_map(&[("host-variant", "/usr/share/guests/self")]);
        let err = validate_guest_image_entries("host-variant", &map, None)
            .expect_err("self-reference must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("host-variant") && msg.contains("itself"),
            "error must explain the self-reference, got: {msg}"
        );
    }

    /// `:` and newline are reserved as `GUEST_IMAGES` field/record separators and must be
    /// rejected in guest names.
    #[test]
    fn test_validate_guest_image_entries_rejects_invalid_names() {
        for bad_name in [":sneaky", "with:colon", "new\nline", ""] {
            let map = guest_map(&[(bad_name, "/usr/share/guests/foo")]);
            let err = validate_guest_image_entries("host", &map, None).expect_err(&format!(
                "guest name {bad_name:?} should have been rejected but was accepted"
            ));
            let msg = format!("{err}");
            assert!(
                msg.contains("not permitted in a guest variant name"),
                "bad name {bad_name:?} should produce InvalidName error, got: {msg}"
            );
        }
    }

    /// Install paths must be absolute and free of `:`, newline, or `..` components.
    #[test]
    fn test_validate_guest_image_entries_rejects_invalid_paths() {
        // Non-absolute -> distinct error variant; we just check it fails.
        let map = guest_map(&[("g", "relative/path")]);
        let err = validate_guest_image_entries("host", &map, None)
            .expect_err("non-absolute path must be rejected");
        assert!(format!("{err}").contains("must be absolute"));

        // `:` injection
        let map = guest_map(&[("g", "/usr/share:trick")]);
        assert!(
            validate_guest_image_entries("host", &map, None).is_err(),
            "':' in path must be rejected"
        );

        // newline injection
        let map = guest_map(&[("g", "/usr/share/foo\nbar")]);
        assert!(
            validate_guest_image_entries("host", &map, None).is_err(),
            "newline in path must be rejected"
        );

        // `..` traversal
        let map = guest_map(&[("g", "/usr/share/../etc/passwd")]);
        let err = validate_guest_image_entries("host", &map, None)
            .expect_err("'..' in path must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("must not contain '..' components"),
            "'..' rejection should explain itself, got: {msg}"
        );
    }

    /// `image-format = "eif"` combined with a non-empty `guest-images` map must be rejected
    /// up front: the EIF pipeline (`rpm2eif`) does not consume the `GUEST_IMAGES` build-arg,
    /// so silently accepting the combination would produce a host image with no embedded
    /// guest artifacts. This mirrors the pre-existing `EifRepackUnsupported` check for
    /// `img2img`. See PR#669 review comments from `sky1122` and `vigh-m`.
    #[test]
    fn test_validate_guest_image_entries_rejects_eif_image_format() {
        let map = guest_map(&[("inner-variant", "/usr/share/bottlerocket/guests/inner")]);
        let err = validate_guest_image_entries("host-variant", &map, Some(&ImageFormat::Eif))
            .expect_err("EIF host variant must not declare guest-images");
        let msg = format!("{err}");
        assert!(
            msg.contains("host-variant") && msg.contains("eif"),
            "error must name the variant and the offending image-format, got: {msg}"
        );
    }

    /// Non-EIF image formats (raw/qcow2/vmdk) — and the absence of an explicit
    /// `image-format` — must all accept `guest-images` unchanged, so the new EIF gate does
    /// not regress the disk-image path.
    #[test]
    fn test_validate_guest_image_entries_accepts_disk_image_formats() {
        let map = guest_map(&[("inner-variant", "/usr/share/bottlerocket/guests/inner")]);
        for fmt in [
            None,
            Some(&ImageFormat::Raw),
            Some(&ImageFormat::Qcow2),
            Some(&ImageFormat::Vmdk),
        ] {
            validate_guest_image_entries("host-variant", &map, fmt).unwrap_or_else(|e| {
                panic!("image-format {fmt:?} must accept guest-images, got error: {e}")
            });
        }
    }

    /// Validation runs at the manifest level (`Manifest::guest_image_variant_deps`), so the
    /// graph-aware happy path is exercised against the `guest-images-kit` fixture: declaring
    /// `inner-variant` (a direct build-dep) succeeds and round-trips the install path.
    #[test]
    fn test_guest_image_variant_deps_happy_path() {
        let manifest_path = guest_kit_manifest("wrapper-variant");
        let temp_dir = TempDir::new().unwrap();
        let cargo_metadata_path = guest_kit_cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, cargo_metadata_path).unwrap();
        let deps = manifest
            .guest_image_variant_deps()
            .expect("wrapper-variant -> inner-variant is a valid direct build-dep");
        assert_eq!(deps.len(), 1);
        let (guest, path) = &deps[0];
        assert_eq!(guest, "inner-variant");
        assert_eq!(path, &PathBuf::from("/usr/share/bottlerocket/guests/inner"));
    }

    /// A leaf variant (`inner-variant`) declares no `guest-images`, so the call returns an
    /// empty list rather than erroring.
    #[test]
    fn test_guest_image_variant_deps_returns_empty_when_unset() {
        let manifest_path = guest_kit_manifest("inner-variant");
        let temp_dir = TempDir::new().unwrap();
        let cargo_metadata_path = guest_kit_cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, cargo_metadata_path).unwrap();
        let deps = manifest.guest_image_variant_deps().unwrap();
        assert!(deps.is_empty());
    }

    /// `guest-kit-manifest` resolves to a `Cargo.toml` under the `guest-images-kit` fixture.
    /// Subdir is derived from the name's prefix/suffix the same way `cargo_manifest` does it.
    #[allow(clippy::disallowed_methods)] // canonicalize required to match cargo-metadata's paths
    fn guest_kit_manifest(name: &str) -> PathBuf {
        let subdir = if name.starts_with("pkg-") || name.ends_with("-pkg") {
            "packages"
        } else if name.ends_with("kit") {
            "kits"
        } else {
            "variants"
        };
        let path = test_projects_dir()
            .join("guest-images-kit")
            .join(subdir)
            .join(name)
            .join("Cargo.toml");
        path.canonicalize()
            .unwrap_or_else(|_| panic!("unable to canonicalize {}", path.display()))
    }

    fn guest_kit_cargo_metadata_path(temp_dir: &TempDir) -> PathBuf {
        let output_path = temp_dir.path().join("cargo_metadata.json");
        let output = MetadataCommand::new()
            .manifest_path(
                test_projects_dir()
                    .join("guest-images-kit")
                    .join("Cargo.toml"),
            )
            .current_dir(temp_dir.path())
            .other_options(["--locked", "--frozen", "--offline"])
            .cargo_command()
            .output()
            .unwrap();

        if !output.status.success() {
            panic!("cargo command failed {:?}", output)
        }

        fs::write(&output_path, output.stdout).unwrap();
        output_path
    }

    /// Minimal fixture under `tests/projects/guest-images-graph/` whose only purpose is to
    /// exercise the graph-aware checks in `guest_image_variant_deps`. Layout:
    ///
    /// ```text
    /// host (variant)
    ///   ├─ [build-deps] direct (variant)         <- valid guest
    ///   └─ [build-deps] middle-kit (kit)
    ///         ├─ [deps] transitive (variant)     <- reachable but NOT a direct build-dep
    ///         └─ [deps] some-pkg (package)
    /// ```
    #[allow(clippy::disallowed_methods)] // canonicalize required to match cargo-metadata's paths
    fn graph_fixture_manifest(name: &str) -> PathBuf {
        let subdir = if name.starts_with("pkg-") || name.ends_with("-pkg") {
            "packages"
        } else if name.ends_with("kit") {
            "kits"
        } else {
            "variants"
        };
        let path = test_projects_dir()
            .join("guest-images-graph")
            .join(subdir)
            .join(name)
            .join("Cargo.toml");
        path.canonicalize()
            .unwrap_or_else(|_| panic!("unable to canonicalize {}", path.display()))
    }

    fn graph_fixture_cargo_metadata_path(temp_dir: &TempDir) -> PathBuf {
        let output_path = temp_dir.path().join("cargo_metadata.json");
        let output = MetadataCommand::new()
            .manifest_path(
                test_projects_dir()
                    .join("guest-images-graph")
                    .join("Cargo.toml"),
            )
            .current_dir(temp_dir.path())
            .other_options(["--locked", "--frozen", "--offline"])
            .cargo_command()
            .output()
            .unwrap();

        if !output.status.success() {
            panic!("cargo command failed {:?}", output)
        }

        fs::write(&output_path, output.stdout).unwrap();
        output_path
    }

    /// `host` declares `direct` (a direct build-dep variant) under `guest-images`. This is the
    /// only acceptable shape; the graph fixture's `host/Cargo.toml` already configures it that
    /// way, so `Manifest::new` + `guest_image_variant_deps` should round-trip cleanly.
    #[test]
    fn test_guest_image_variant_deps_accepts_direct_build_dep() {
        let manifest_path = graph_fixture_manifest("host");
        let temp_dir = TempDir::new().unwrap();
        let metadata_path = graph_fixture_cargo_metadata_path(&temp_dir);
        let manifest = Manifest::new(manifest_path, metadata_path).unwrap();
        let deps = manifest
            .guest_image_variant_deps()
            .expect("direct build-dep variant must be accepted");
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].0, "direct");
    }

    /// Replacing `host`'s `guest-images` with `transitive` (which is reachable only via
    /// `middle-kit`'s normal `[dependencies]`, not as a direct build-dep of `host`) must
    /// produce the actionable `GuestImagesNotDirectBuildDep` error rather than the generic
    /// "missing" variant.
    #[test]
    fn test_guest_image_variant_deps_rejects_transitive_only_dep() {
        // Build a synthetic `host/Cargo.toml` that points at `transitive` instead of `direct`,
        // place it next to the real fixture in a temp dir, and re-resolve metadata. We can't
        // rewrite the source fixture in-place because other tests rely on the original shape
        // and parallel test execution would race.
        let temp = TempDir::new().unwrap();
        let workspace_root = stage_graph_fixture_with_host_override(
            &temp,
            r#"
[package]
name = "host"
version = "0.1.0"
edition = "2021"
publish = false

[lib]
path = "../../lib.rs"

[package.metadata.build-variant]
included-packages = []
kernel-parameters = []

[package.metadata.build-variant.guest-images]
transitive = "/usr/share/bottlerocket/guests/transitive"

[build-dependencies]
direct = { path = "../direct" }
middle-kit = { path = "../../kits/middle-kit" }
"#,
        );
        // `cargo metadata` emits canonical paths on macOS (where `/var` is a symlink to
        // `/private/var`); `Manifest::new` matches the manifest to the graph node by path,
        // so the test path must be canonicalized to match. This is precisely the case where
        // symlink resolution is intended, so opt out of the workspace-wide `canonicalize`
        // lint locally rather than papering over the mismatch with `path_absolutize`.
        #[allow(clippy::disallowed_methods)]
        let manifest_path = workspace_root
            .join("variants/host/Cargo.toml")
            .canonicalize()
            .unwrap();
        let metadata_path = run_cargo_metadata(&workspace_root, &temp);
        let manifest = Manifest::new(manifest_path, metadata_path).unwrap();
        let err = manifest
            .guest_image_variant_deps()
            .expect_err("transitive-only variant must be rejected");
        let msg = format!("{err}");
        assert!(
            msg.contains("transitive dependency, not a direct"),
            "error must explain that the dep is transitive, got: {msg}"
        );
        assert!(msg.contains("transitive"), "error must name the guest");
    }

    /// Naming a crate that doesn't exist (or isn't a variant) under `guest-images` produces
    /// `GuestImagesMissingBuildDep` rather than `NotDirectBuildDep`. We exercise both
    /// sub-cases: an unknown name (`nonexistent`) and a known but non-variant crate
    /// (`some-pkg`, a package). Both must point the user at `[build-dependencies]`.
    #[test]
    fn test_guest_image_variant_deps_rejects_unknown_or_non_variant_dep() {
        for unknown in ["nonexistent", "some-pkg"] {
            let temp = TempDir::new().unwrap();
            let workspace_root = stage_graph_fixture_with_host_override(
                &temp,
                &format!(
                    r#"
[package]
name = "host"
version = "0.1.0"
edition = "2021"
publish = false

[lib]
path = "../../lib.rs"

[package.metadata.build-variant]
included-packages = []
kernel-parameters = []

[package.metadata.build-variant.guest-images]
{unknown} = "/usr/share/bottlerocket/guests/x"

[build-dependencies]
direct = {{ path = "../direct" }}
middle-kit = {{ path = "../../kits/middle-kit" }}
"#
                ),
            );
            // See `test_guest_image_variant_deps_rejects_transitive_only_dep` for why
            // symlink resolution is needed here (macOS `/var` -> `/private/var`).
            #[allow(clippy::disallowed_methods)]
            let manifest_path = workspace_root
                .join("variants/host/Cargo.toml")
                .canonicalize()
                .unwrap();
            let metadata_path = run_cargo_metadata(&workspace_root, &temp);
            let manifest = Manifest::new(manifest_path, metadata_path).unwrap();
            let err = manifest
                .guest_image_variant_deps()
                .expect_err(&format!("guest {unknown:?} must be rejected"));
            let msg = format!("{err}");
            assert!(
                msg.contains("not in its build-dependencies"),
                "guest {unknown:?} should produce MissingBuildDep error, got: {msg}"
            );
        }
    }

    /// Stage a copy of the `guest-images-graph` fixture into `temp`, replacing
    /// `variants/host/Cargo.toml` with the supplied contents. Returns the staged workspace
    /// root. Other crates and the workspace `Cargo.toml` are copied verbatim so cargo can
    /// resolve a fresh metadata dump rooted there.
    fn stage_graph_fixture_with_host_override(temp: &TempDir, host_toml: &str) -> PathBuf {
        let src = test_projects_dir().join("guest-images-graph");
        let dst = temp.path().join("guest-images-graph");
        copy_dir_all(&src, &dst);
        fs::write(dst.join("variants/host/Cargo.toml"), host_toml).unwrap();
        // Force regeneration of Cargo.lock from the modified manifest. The original lockfile
        // still works (host's deps are unchanged) but if a future test mutates the dep graph,
        // dropping the lock keeps things honest.
        let _ = fs::remove_file(dst.join("Cargo.lock"));
        dst
    }

    fn run_cargo_metadata(workspace_root: &Path, temp: &TempDir) -> PathBuf {
        let output_path = temp.path().join("cargo_metadata.json");
        let output = MetadataCommand::new()
            .manifest_path(workspace_root.join("Cargo.toml"))
            .current_dir(temp.path())
            .cargo_command()
            .output()
            .unwrap();
        if !output.status.success() {
            panic!("cargo metadata failed: {output:?}");
        }
        fs::write(&output_path, output.stdout).unwrap();
        output_path
    }

    fn copy_dir_all(src: &Path, dst: &Path) {
        fs::create_dir_all(dst).unwrap();
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let from = entry.path();
            let to = dst.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir_all(&from, &to);
            } else {
                fs::copy(&from, &to).unwrap();
            }
        }
    }
}
