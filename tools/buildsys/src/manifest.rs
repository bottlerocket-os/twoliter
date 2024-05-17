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
This can be `raw` (the default), `vmdk`, or `qcow2`.
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
to conditionally use or exclude certain firmware-level features in variants.

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

`unified-cgroup-hierarchy` makes systemd set up a unified cgroup hierarchy on
boot, i.e. the host will use cgroup v2 by default. This feature flag allows
old variants to continue booting with cgroup v1 and new variants to move to
cgroup v2, while users will still be able to override the default via command
line arguments set in the boot configuration.
```ignore
[package.metadata.build-variant.image-features]
unified-cgroup-hierarchy = true
```

`xfs-data-partition` changes the filesystem for the data partition from ext4 to xfs. The
default will remain ext4 and xfs is opt-in.

```ignore
[package.metadata.build-variant.image-features]
xfs-data-partition = true
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

*/

mod error;

use crate::BuildType;
use guppy::graph::{DependencyDirection, PackageGraph, PackageLink, PackageMetadata};
use guppy::{CargoMetadata, PackageId};
use serde::{Deserialize, Serialize};
use snafu::{OptionExt, ResultExt, Snafu};
use std::cmp::max;
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::fmt::{self, Display};
use std::fs;
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

/// The nested structures here are somewhat complex, but they make it trivial
/// to deserialize the structure we expect to find in the manifest.
#[derive(Deserialize, Debug)]
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
        self.build_variant().and_then(|b| {
            b.image_features
                .as_ref()
                .map(|m| m.iter().filter(|(_k, v)| **v).map(|(k, _v)| *k).collect())
        })
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
            Err(Error(error::UnknownManifestTypeSnafu {}.build()))
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
    match manifest_type {
        BuildType::Package => metadata_table.get("build-package").is_some(),
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

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
struct Package {
    name: String,
    metadata: Option<Metadata>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
struct Metadata {
    build_package: Option<BuildPackage>,
    build_kit: Option<BuildKit>,
    build_variant: Option<BuildVariant>,
}

#[derive(Deserialize, Debug)]
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

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)]
pub struct BuildKit {
    pub included_packages: Option<Vec<String>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
#[serde(untagged)]
pub enum VariantSensitivity {
    Any(bool),
    Specific(SensitivityType),
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum SensitivityType {
    Platform,
    Runtime,
    Family,
    Flavor,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct BuildVariant {
    pub included_packages: Option<Vec<String>>,
    pub image_format: Option<ImageFormat>,
    #[serde(default)]
    pub image_layout: ImageLayout,
    pub supported_arches: Option<HashSet<SupportedArch>>,
    pub kernel_parameters: Option<Vec<String>>,
    pub image_features: Option<HashMap<ImageFeature, bool>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
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
pub struct ImageLayout {
    #[serde(default = "ImageLayout::default_os_image_size_gib")]
    pub os_image_size_gib: ImageSize,
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
static DEFAULT_DATA_IMAGE_SIZE_GIB: ImageSize = ImageSize(1);
static DEFAULT_PUBLISH_IMAGE_SIZE_HINT_GIB: ImageSize = ImageSize(22);
static DEFAULT_PARTITION_PLAN: PartitionPlan = PartitionPlan::Split;

impl ImageLayout {
    fn default_os_image_size_gib() -> ImageSize {
        DEFAULT_OS_IMAGE_SIZE_GIB
    }

    fn default_data_image_size_gib() -> ImageSize {
        DEFAULT_DATA_IMAGE_SIZE_GIB
    }

    fn default_publish_image_size_hint_gib() -> ImageSize {
        DEFAULT_PUBLISH_IMAGE_SIZE_HINT_GIB
    }

    fn default_partition_plan() -> PartitionPlan {
        DEFAULT_PARTITION_PLAN
    }

    // At publish time we will need specific sizes for the OS image and the (optional) data image.
    // The sizes returned by this function depend on the image layout, and whether the publish
    // image hint is larger than the required minimum size.
    pub fn publish_image_sizes_gib(&self) -> (i32, i32) {
        let os_image_base_size_gib = self.os_image_size_gib.0;
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
            os_image_size_gib: Self::default_os_image_size_gib(),
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
    UnifiedCgroupHierarchy,
    XfsDataPartition,
    UefiSecureBoot,
    Fips,
}

impl TryFrom<String> for ImageFeature {
    type Error = Error;
    fn try_from(s: String) -> Result<Self> {
        match s.as_str() {
            "grub-set-private-var" => Ok(ImageFeature::GrubSetPrivateVar),
            "systemd-networkd" => Ok(ImageFeature::SystemdNetworkd),
            "unified-cgroup-hierarchy" => Ok(ImageFeature::UnifiedCgroupHierarchy),
            "xfs-data-partition" => Ok(ImageFeature::XfsDataPartition),
            "uefi-secure-boot" => Ok(ImageFeature::UefiSecureBoot),
            "fips" => Ok(ImageFeature::Fips),
            _ => error::ParseImageFeatureSnafu { what: s }.fail()?,
        }
    }
}

impl fmt::Display for ImageFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageFeature::GrubSetPrivateVar => write!(f, "GRUB_SET_PRIVATE_VAR"),
            ImageFeature::SystemdNetworkd => write!(f, "SYSTEMD_NETWORKD"),
            ImageFeature::UnifiedCgroupHierarchy => write!(f, "UNIFIED_CGROUP_HIERARCHY"),
            ImageFeature::XfsDataPartition => write!(f, "XFS_DATA_PARTITION"),
            ImageFeature::UefiSecureBoot => write!(f, "UEFI_SECURE_BOOT"),
            ImageFeature::Fips => write!(f, "FIPS"),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub enum BundleModule {
    Go,
}

#[derive(Deserialize, Debug)]
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
        let expected = vec!["pkg-a-renamed".to_string()];
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
}
