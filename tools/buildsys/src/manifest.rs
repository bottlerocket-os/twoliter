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
default seed for an EIF variant: `first-party-stack`, `in-place-updates`,
`host-containers`. The following combinations are rejected at build
time (in the same style as `first-party-stack = false`):
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

`first-party-stack` controls whether the Bottlerocket-provided software stack — the
orchestrator integration (kubelet, containerd, host-containers, admin-containers), the
settings system (apiserver, datastore, settings rendering, migrations), in-place updates, and
the BOTTLEROCKET-PRIVATE / BOTTLEROCKET-DATA partitions — is built into the image. The
default is `true`.

When `first-party-stack = false`, the image is reduced to the kernel, base userspace,
dm-verity-protected rootfs, and the update mechanism. The OS image has a single bank of OS
partitions (BIOS-BOOT, EFI-SYSTEM, BOOT-A, ROOT-A, HASH-A) with no `RESERVED-A` partition;
ROOT-A absorbs the slack. The default OS image size becomes 1 GiB unless `os-image-size-gib`
is explicitly set. The variant author owns all system configuration; if Bottlerocket
components that expect persistent storage at `partlabel=BOTTLEROCKET-DATA` are shipped, such
a volume may optionally be attached at runtime.

`first-party-stack = false` requires `partition-plan = "unified"`, and is incompatible with
`uefi-secure-boot`, `encrypted-storage`, `in-place-updates`, and `host-containers`; the build
will fail fast in those cases. Setting `first-party-stack = false` also turns off the silent
defaults for `in-place-updates` and `host-containers` so that, once `partition-plan = "unified"`
is set, toggling `first-party-stack = false` is sufficient to produce a stripped-down image.

```ignore
[package.metadata.build-variant.image-features]
first-party-stack = false

[package.metadata.build-variant.image-layout]
partition-plan = "unified"
```

*/

mod error;

use crate::BuildType;
use buildsys_config::EXTERNAL_KIT_METADATA;
use guppy::graph::{DependencyDirection, PackageGraph, PackageLink, PackageMetadata};
use guppy::{CargoMetadata, PackageId};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu};
use std::cmp::max;
use std::collections::{HashMap, HashSet};
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
            .query_forward(ids.into_iter())
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
            .query_forward(ids.into_iter())
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
    /// `package.metadata.build-package.package-name` key, it is returned, otherwise the Cargo
    /// manifest name is returned from `package.name`.
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

    /// Convenience method to return the enabled image features for this variant.
    pub fn image_features(&self) -> Option<HashSet<ImageFeature>> {
        let variant = self.build_variant()?;
        // If the user explicitly disabled `first-party-stack`, drop the silent
        // defaults for `in-place-updates` and `host-containers` (and
        // `first-party-stack` itself). An explicit `in-place-updates = true`
        // is still rejected later by the validator.
        //
        // `image-format = "eif"` is treated the same as `first-party-stack =
        // false` for seeding purposes: an EIF is inherently a stripped-down
        // single-bank image, so shipping the second bank / host-containers /
        // BOTTLEROCKET-DATA subsystems by default only invites silent
        // misconfiguration when the Dockerfile drops those flags on the way
        // to `rpm2eif`. The validator will still reject an *explicit*
        // conflicting feature set with a clear error.
        let first_party_stack_explicitly_disabled = variant
            .image_features
            .as_ref()
            .and_then(|m| m.get(&ImageFeature::FirstPartyStack))
            .copied()
            .map(|enabled| !enabled)
            .unwrap_or(false);
        let is_eif = matches!(variant.image_format, Some(ImageFormat::Eif));
        let mut features = if first_party_stack_explicitly_disabled || is_eif {
            HashSet::from([ImageFeature::ExternalKmodDevelopment])
        } else {
            HashSet::from([
                ImageFeature::FirstPartyStack,
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
    let package_name_override = pkg_metadata
        .metadata_table()
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
    // Variant attribute overrides
    pub platform: Option<String>,
    pub runtime: Option<String>,
    pub family: Option<String>,
    pub version: Option<String>,
    pub flavor: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Eif,
    Qcow2,
    Raw,
    Vmdk,
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
/// Default OS image size when `first-party-stack = false` and the manifest
/// does not specify `os-image-size-gib`.
static DEFAULT_NO_FIRST_PARTY_OS_IMAGE_SIZE_GIB: ImageSize = ImageSize(1);
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
    FirstPartyStack,
}

const EXPERIMENTAL_IMAGE_FEATURES: &[&ImageFeature] = &[&ImageFeature::EncryptedStorage];

const DEPRECATED_IMAGE_FEATURES: &[&ImageFeature] = &[
    &ImageFeature::GrubSetPrivateVar,
    &ImageFeature::SystemdNetworkd,
];

/// Returns an [`ImageLayout`] with fields adjusted for the given feature set
/// and image format.
///
/// - If `first-party-stack` is disabled (or the image format is `eif`, which
///   is inherently first-party-stack-free) and the manifest did not
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
/// the pre-existing `first-party-stack`-only behavior.
pub fn resolved_image_layout(
    layout: &ImageLayout,
    features: &HashSet<ImageFeature>,
    image_format: Option<&ImageFormat>,
) -> ImageLayout {
    let mut resolved = *layout;
    let is_eif = matches!(image_format, Some(ImageFormat::Eif));
    let stripped = !features.contains(&ImageFeature::FirstPartyStack) || is_eif;
    if stripped && !layout.os_image_size_gib_was_set() {
        resolved.os_image_size_gib = Some(DEFAULT_NO_FIRST_PARTY_OS_IMAGE_SIZE_GIB);
    }
    if is_eif {
        resolved.partition_plan = PartitionPlan::Unified;
    }
    resolved
}

/// Validate that the requested image features and layout are compatible.
///
/// This enforces that disabling `first-party-stack` is not combined with any
/// of `uefi-secure-boot`, `encrypted-storage`, `in-place-updates`,
/// `host-containers`, `xfs-data-partition`, or with a `split` partition plan.
///
/// An `image-format = "eif"` build is subject to the same constraints
/// regardless of the `first-party-stack` toggle: an EIF is a single-bank,
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
    // With `first-party-stack` enabled *and* a non-EIF format, every
    // combination is legal.
    if features.contains(&ImageFeature::FirstPartyStack) && !is_eif {
        return Ok(());
    }

    // Explain conflicts as either a first-party-stack violation or an EIF
    // violation, whichever applies. EIF takes precedence in the message
    // because it's the more surprising case (the format silently strips
    // features rather than an explicit `first-party-stack = false`).
    let context: &str = if is_eif {
        "`image-format = \"eif\"`"
    } else {
        "`first-party-stack = false`"
    };
    let reason_secure_boot: String = if is_eif {
        "secure boot is not supported by the EIF pipeline; `rpm2eif` does not sign shim/grub/vmlinuz".to_string()
    } else {
        "secure boot relies on signed first-party artifacts that are not present when `first-party-stack = false`".to_string()
    };
    let reason_encrypted_storage: String = if is_eif {
        "encrypted storage requires the BOTTLEROCKET-PRIVATE/DATA partitions, which an EIF image does not have".to_string()
    } else {
        "encrypted storage requires the BOTTLEROCKET-PRIVATE/DATA partitions, which are not built when `first-party-stack = false`".to_string()
    };
    let reason_ipu: String = if is_eif {
        "in-place updates require two banks of OS partitions; an EIF ships a single ROOT-A/HASH-A pair".to_string()
    } else {
        "in-place updates require two banks of OS partitions, which are not built when `first-party-stack = false`".to_string()
    };
    let reason_host_containers: String = if is_eif {
        "host-containers require the BOTTLEROCKET-DATA partition, which an EIF image does not have"
            .to_string()
    } else {
        "host-containers require the BOTTLEROCKET-DATA partition, which is not built when `first-party-stack = false`".to_string()
    };
    let reason_first_party_stack =
        "the EIF pipeline is inherently stripped down; `rpm2eif` requires `first-party-stack = false`";
    let reason_xfs_data_partition: String = if is_eif {
        "xfs-data-partition requires the BOTTLEROCKET-DATA partition, which an EIF image does not have".to_string()
    } else {
        "xfs-data-partition requires the BOTTLEROCKET-DATA partition, which is not built when `first-party-stack = false`".to_string()
    };

    let conflicts: &[(ImageFeature, &str, &str, bool)] = &[
        (
            ImageFeature::FirstPartyStack,
            "first-party-stack",
            reason_first_party_stack,
            true,
        ),
        (
            ImageFeature::XfsDataPartition,
            "xfs-data-partition",
            &reason_xfs_data_partition,
            false,
        ),
        (
            ImageFeature::UefiSecureBoot,
            "uefi-secure-boot",
            &reason_secure_boot,
            false,
        ),
        (
            ImageFeature::EncryptedStorage,
            "encrypted-storage",
            &reason_encrypted_storage,
            false,
        ),
        (
            ImageFeature::InPlaceUpdates,
            "in-place-updates",
            &reason_ipu,
            false,
        ),
        (
            ImageFeature::HostContainers,
            "host-containers",
            &reason_host_containers,
            false,
        ),
    ];

    // Collect all conflicts in a deterministic order so the user sees the
    // complete list in a single build cycle, rather than playing whack-a-mole.
    // Use the kebab-case manifest key in messages to match the TOML the user
    // wrote and the error messages produced by the shell-side validators.
    let mut conflict_messages: Vec<String> = conflicts
        .iter()
        .filter(|(feature, _, _, eif_only)| features.contains(feature) && (!eif_only || is_eif))
        .map(|(_, name, reason, _)| format!("`{name}`: {reason}"))
        .collect();

    if matches!(layout.partition_plan, PartitionPlan::Split) {
        let reason = if is_eif {
            "`partition-plan=split`: an EIF is a single unified disk; the split \
             layout has no meaning"
        } else {
            "`partition-plan=split`: when `first-party-stack = false`, the image uses a \
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
            "first-party-stack" => Ok(ImageFeature::FirstPartyStack),
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
/// | Rust enum variant                  | UpperCamelCase       | `FirstPartyStack` |
/// | Manifest TOML key (TryFrom impl)   | kebab-case           | `first-party-stack` |
/// | Display impl (env var name)        | UPPER_SNAKE_CASE     | `FIRST_PARTY_STACK` |
/// | Dockerfile `--build-arg` value     | `1` (truthy) or `yes` | `FIRST_PARTY_STACK=yes` |
/// | Shell script `--with-X=` value     | `yes` / `no`         | `--with-first-party-stack=no` |
/// | Runtime `image-features.env` value | `true` / `false`     | `FIRST_PARTY_STACK=true` |
///
/// Each layer's representation is intentional: the Dockerfile uses `1` so
/// that bash's `${VAR:+...}` expansion can convert presence to a flag, the
/// shell scripts use `yes`/`no` to be readable in build logs, and the
/// runtime env file uses `true`/`false` to be parseable by configuration
/// loaders. `FirstPartyStack` is the exception: since it is default-true
/// and inverted, its build-arg is `yes` so the Dockerfile can pipe the
/// value directly to `--with-first-party-stack=`. See
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
            ImageFeature::FirstPartyStack => write!(f, "FIRST_PARTY_STACK"),
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

    fn first_party_stack_disabled_layout(os_size: Option<u16>, plan: PartitionPlan) -> ImageLayout {
        ImageLayout {
            os_image_size_gib: os_size.map(ImageSize),
            data_image_size_gib: DEFAULT_DATA_IMAGE_SIZE_GIB,
            publish_image_size_hint_gib: DEFAULT_PUBLISH_IMAGE_SIZE_HINT_GIB,
            partition_plan: plan,
        }
    }

    #[test]
    fn first_party_stack_disabled_default_os_size_is_one_gib() {
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Unified);
        // No FirstPartyStack in the set → stripped image, default 1 GiB.
        let features: HashSet<ImageFeature> = HashSet::new();
        let resolved = resolved_image_layout(&layout, &features, None);
        assert_eq!(resolved.os_image_size_gib().0, 1);
    }

    #[test]
    fn first_party_stack_disabled_respects_explicit_os_size() {
        let layout = first_party_stack_disabled_layout(Some(4), PartitionPlan::Unified);
        let features: HashSet<ImageFeature> = HashSet::new();
        let resolved = resolved_image_layout(&layout, &features, None);
        assert_eq!(resolved.os_image_size_gib().0, 4);
    }

    #[test]
    fn first_party_stack_default_os_size_is_two_gib() {
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::FirstPartyStack]);
        let resolved = resolved_image_layout(&layout, &features, None);
        assert_eq!(resolved.os_image_size_gib().0, 2);
    }

    #[test]
    fn first_party_stack_disabled_alone_passes_validation() {
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Unified);
        let features: HashSet<ImageFeature> = HashSet::new();
        validate_image_features(&features, &layout, None)
            .expect("first-party-stack=false + unified should validate");
    }

    #[test]
    fn first_party_stack_disabled_rejects_secure_boot() {
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::UefiSecureBoot]);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn first_party_stack_disabled_rejects_encrypted_storage() {
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::EncryptedStorage]);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn first_party_stack_disabled_rejects_in_place_updates() {
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::InPlaceUpdates]);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn first_party_stack_disabled_rejects_split_partition_plan() {
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Split);
        let features: HashSet<ImageFeature> = HashSet::new();
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn validation_is_noop_when_first_party_stack_enabled() {
        // With `first-party-stack` enabled, the validator should accept any
        // combination.
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Split);
        let features = HashSet::from([
            ImageFeature::FirstPartyStack,
            ImageFeature::UefiSecureBoot,
            ImageFeature::EncryptedStorage,
            ImageFeature::InPlaceUpdates,
        ]);
        validate_image_features(&features, &layout, None)
            .expect("validation should be no-op with first-party-stack enabled");
    }

    #[test]
    fn first_party_stack_disabled_rejects_host_containers() {
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::HostContainers]);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn first_party_stack_disabled_rejects_xfs_data_partition() {
        // `first-party-stack = false` skips building the BOTTLEROCKET-DATA
        // partition, so opting into `xfs-data-partition` on top of it would
        // silently do nothing. Reject the combination at build time.
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Unified);
        let features = HashSet::from([ImageFeature::XfsDataPartition]);
        let err = validate_image_features(&features, &layout, None)
            .expect_err("first-party-stack=false + xfs-data-partition must fail validation");
        let msg = format!("{err}");
        assert!(
            msg.contains("xfs-data-partition"),
            "missing 'xfs-data-partition' in: {msg}",
        );
        assert!(
            msg.contains("first-party-stack = false"),
            "message should reference first-party-stack=false: {msg}",
        );
    }

    #[test]
    fn first_party_stack_disabled_reports_all_conflicts_at_once() {
        // The validator should report every conflict in a single error so the
        // user does not have to fix them one at a time across multiple builds.
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Split);
        let features = HashSet::from([
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
    fn image_features_first_party_stack_disabled_drops_silent_defaults() {
        // `first-party-stack = false` alone should drop the silent
        // `in-place-updates` and `host-containers` defaults (and remove
        // `first-party-stack` itself from the seed).
        let toml = variant_manifest_with_image_features(
            "[package.metadata.build-variant.image-features]\nfirst-party-stack = false",
        );
        let info = manifest_info_from_toml(&toml);
        let features = info.image_features().expect("variant has image-features");
        assert!(!features.contains(&ImageFeature::FirstPartyStack));
        assert!(features.contains(&ImageFeature::ExternalKmodDevelopment));
        assert!(!features.contains(&ImageFeature::InPlaceUpdates));
        assert!(!features.contains(&ImageFeature::HostContainers));
    }

    #[test]
    fn image_features_first_party_stack_disabled_with_host_containers_keeps_both() {
        // The seed-set logic adds host-containers back if it's explicitly set
        // to `true`, so that the validator can later reject the combination
        // with a clear error rather than silently producing a confused image.
        let toml = variant_manifest_with_image_features(
            "[package.metadata.build-variant.image-features]\n\
             first-party-stack = false\n\
             host-containers = true",
        );
        let info = manifest_info_from_toml(&toml);
        let features = info.image_features().expect("variant has image-features");
        assert!(!features.contains(&ImageFeature::FirstPartyStack));
        assert!(features.contains(&ImageFeature::HostContainers));
        // Validator should reject this combination.
        let layout = first_party_stack_disabled_layout(None, PartitionPlan::Unified);
        assert!(validate_image_features(&features, &layout, None).is_err());
    }

    #[test]
    fn image_features_default_seed_includes_first_party_stack() {
        // No `image-features` at all → the default seed includes
        // `first-party-stack` along with the historical silent defaults.
        let toml = r#"
[package]
name = "test-variant"

[package.metadata.build-variant]
"#;
        let info = manifest_info_from_toml(toml);
        let features = info
            .image_features()
            .expect("variant has build-variant section");
        assert!(features.contains(&ImageFeature::FirstPartyStack));
        assert!(features.contains(&ImageFeature::InPlaceUpdates));
        assert!(features.contains(&ImageFeature::HostContainers));
        assert!(features.contains(&ImageFeature::ExternalKmodDevelopment));
    }

    #[test]
    fn image_features_explicit_first_party_stack_true_preserves_defaults() {
        // Explicit `first-party-stack = true` is a no-op vs. the default; the
        // historical silent defaults remain.
        let toml = variant_manifest_with_image_features(
            "[package.metadata.build-variant.image-features]\nfirst-party-stack = true",
        );
        let info = manifest_info_from_toml(&toml);
        let features = info.image_features().expect("variant has image-features");
        assert!(features.contains(&ImageFeature::FirstPartyStack));
        assert!(features.contains(&ImageFeature::InPlaceUpdates));
        assert!(features.contains(&ImageFeature::HostContainers));
        assert!(features.contains(&ImageFeature::ExternalKmodDevelopment));
    }

    #[test]
    fn image_features_omitted_uses_default_image() {
        // A variant that omits the `[image-features]` section entirely
        // produces the standard Bottlerocket image: `first-party-stack` is in
        // the silent default seed, and the resolved layout uses the historical
        // 2 GiB OS size.
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
    // These mirror the `first_party_stack_disabled_*` tests but with the EIF
    // image format as the trigger instead of an explicit
    // `first-party-stack = false`. EIF is inherently a stripped-down,
    // single-bank image; the validator must reject conflicting features so
    // that misconfiguration surfaces at build time rather than silently
    // producing a broken artifact via `rpm2eif`'s reduced pipeline.
    // ---------------------------------------------------------------------

    #[test]
    fn eif_format_default_os_size_is_one_gib() {
        // Same rationale as `first_party_stack_disabled_default_os_size_is_one_gib`:
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
        // The minimal EIF feature set (just ExternalKmodDevelopment, the
        // only default not stripped) with `partition-plan = unified` should
        // validate successfully.
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::ExternalKmodDevelopment]);
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
    fn eif_format_rejects_first_party_stack_alone() {
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::FirstPartyStack]);
        let err = validate_image_features(&features, &layout, Some(&ImageFormat::Eif))
            .expect_err("EIF + first-party-stack must fail validation");
        assert!(format!("{err}").contains("first-party-stack"));
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
    fn eif_format_rejects_first_party_stack_true() {
        // Even if the user explicitly enables `first-party-stack = true`,
        // `image-format = "eif"` must still reject the conflicting features.
        // Without this the EIF path could silently ignore, e.g.,
        // `in-place-updates = true` and produce an image the author did not
        // ask for.
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Unified,
            ..ImageLayout::default()
        };
        let features = HashSet::from([ImageFeature::FirstPartyStack, ImageFeature::InPlaceUpdates]);
        assert!(
            validate_image_features(&features, &layout, Some(&ImageFormat::Eif)).is_err(),
            "EIF must reject conflicts even when first-party-stack is explicitly enabled"
        );
    }

    #[test]
    fn eif_format_reports_all_conflicts_at_once() {
        // Same "don't play whack-a-mole" guarantee as the first-party-stack
        // validator: reject everything in one build cycle.
        let layout = ImageLayout {
            partition_plan: PartitionPlan::Split,
            ..ImageLayout::default()
        };
        let features = HashSet::from([
            ImageFeature::FirstPartyStack,
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
            "first-party-stack",
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
        // section must NOT get the silent first-party-stack / IPU /
        // host-containers defaults. Otherwise the EIF validator would
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
        assert!(!features.contains(&ImageFeature::FirstPartyStack));
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
}
