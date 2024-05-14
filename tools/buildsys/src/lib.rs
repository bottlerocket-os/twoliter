pub mod manifest;

/// The thing that buildsys is being asked to build.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum BuildType {
    Package,
    Kit,
    Variant,
    Repack,
}
