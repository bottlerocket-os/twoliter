use guppy::PackageId;
use snafu::Snafu;
use std::io;
use std::path::PathBuf;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(super)))]
pub(super) enum Error {
    #[snafu(display("Failed to read cargo_metadata file '{}': {}", path.display(), source))]
    CargoMetadataRead { path: PathBuf, source: io::Error },

    #[snafu(display("Failed to parse cargo_metadata json from '{}': {}", path.display(), source))]
    CargoMetadataParse { path: PathBuf, source: guppy::Error },

    #[snafu(display("Cargo package graph query failed with root '{id}': {source}"))]
    CargoPackageQuerySnafu { id: PackageId, source: guppy::Error },

    #[snafu(display("Package '{id}' has no 'vendor' field in build-kit metadata"))]
    NoKitVendor { id: String },

    #[snafu(display("Failed to create dependency graph from '{}': {}", path.display(), source))]
    GraphBuild { path: PathBuf, source: guppy::Error },

    #[snafu(display("Failed to read manifest file '{}': {}", path.display(), source))]
    ManifestFileRead { path: PathBuf, source: io::Error },

    #[snafu(display("Failed to load manifest file '{}': {}", path.display(), source))]
    ManifestFileLoad {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[snafu(display("Failed to read external kit metadata file '{}': {}", path.display(), source))]
    ExternalKitMetadataFileRead { path: PathBuf, source: io::Error },

    #[snafu(display("Failed to load external kit metadata file '{}': {}", path.display(), source))]
    ExternalKitMetadataLoad {
        path: PathBuf,
        source: serde_json::Error,
    },

    #[snafu(display("Failed to parse image feature '{}'", what))]
    ParseImageFeature { what: String },

    #[snafu(display(
        "The cargo package we are building, '{name}', could not be found in the graph"
    ))]
    RootDependencyMissing { name: String },

    #[snafu(display("{context} is incompatible with: {reason}"))]
    IncompatibleImageFeatures {
        context: &'static str,
        reason: String,
    },

    #[snafu(display(
        "Variant '{name}' declares guest-images for itself. A variant cannot use its \
         own images as a guest."
    ))]
    GuestImagesSelfReference { name: String },

    #[snafu(display(
        "Variant '{name}' declares guest-images path '{}' for guest '{guest}'; the install path \
         must be absolute.",
        path.display(),
    ))]
    GuestImagesPathNotAbsolute {
        name: String,
        guest: String,
        path: PathBuf,
    },

    #[snafu(display(
        "Variant '{name}' declares `guest-images = [\"{guest}\"]` but `{guest}` is not in its \
         build-dependencies; add it under [build-dependencies]."
    ))]
    GuestImagesMissingBuildDep { name: String, guest: String },

    #[snafu(display(
        "Variant '{name}' declares `guest-images = [\"{guest}\"]` but `{guest}` is a transitive \
         dependency, not a direct `[build-dependencies]` entry. Add `{guest}` directly under \
         [build-dependencies] of this variant."
    ))]
    GuestImagesNotDirectBuildDep { name: String, guest: String },

    #[snafu(display(
        "Variant '{name}' declares guest-images key '{guest}', which contains a character that \
         is not permitted in a guest variant name (must be a non-empty crate name without ':' or \
         newline characters)."
    ))]
    GuestImagesInvalidName { name: String, guest: String },

    #[snafu(display(
        "Variant '{name}' declares guest-images path '{}' for guest '{guest}'; the install path \
         must not contain ':' or newline characters, and must not contain '..' components.",
        path.display(),
    ))]
    GuestImagesInvalidPath {
        name: String,
        guest: String,
        path: PathBuf,
    },

    #[snafu(display(
        "Variant '{name}' declares `guest-images` but has `image-format = \"eif\"`. Guest image \
         embedding is only implemented for disk-image formats (raw/qcow2/vmdk); the EIF pipeline \
         (`rpm2eif`) does not honor the `GUEST_IMAGES` build-arg. Remove `guest-images` or switch \
         the host variant to a disk-image format."
    ))]
    GuestImagesUnsupportedImageFormat { name: String },
}
