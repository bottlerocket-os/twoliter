/*!
Packages using the Rust programming language may have upstream tar archives that
include only the source code of the project, but not the source code of any
dependencies. Rust projects use Cargo for dependency management, with dependencies
declared in `Cargo.toml` and locked versions in `Cargo.lock`.

This module extends the functionality of `packages.metadata.build-package.external-files`
and provides the ability to retrieve and vendor dependencies using `cargo vendor`
given a tar archive containing a `Cargo.toml` and `Cargo.lock`.

The vendored output includes both the `vendor/` directory and `.cargo/config.toml`
which configures Cargo to use the vendored dependencies.

 */

pub(crate) mod error {
    pub(crate) use crate::vendormod::error::*;
}

use crate::vendormod::{VendorMod, RUST_CONFIG};
use buildsys::manifest;
use error::Result;
use filetime::FileTime;
use std::path::Path;

pub(crate) struct RustMod;

impl RustMod {
    pub(crate) fn vendor(
        root_dir: &Path,
        package_dir: &Path,
        external_file: &manifest::ExternalFile,
        sdk: &str,
        mtime: FileTime,
    ) -> Result<()> {
        VendorMod::vendor(
            &RUST_CONFIG,
            root_dir,
            package_dir,
            external_file,
            sdk,
            mtime,
        )
    }
}
