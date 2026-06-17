/*!
Packages using the Go programming language may have upstream tar archives that
include only the source code of the project, but not the source code of any
dependencies. The Go programming language promotes the use of "modules" for
dependencies. Projects adopting modules will provide `go.mod` and `go.sum` files.

This Rust module extends the functionality of `packages.metadata.build-package.external-files`
and provides the ability to retrieve and validate dependencies
declared using Go modules given a tar archive containing a `go.mod` and `go.sum`.

The location where dependencies are retrieved from are controlled by the
standard environment variables employed by the Go tool: `GOPROXY`, `GOSUMDB`, and
`GOPRIVATE`. These variables are automatically retrieved from the host environment
when the docker-go script is invoked.

 */

pub(crate) mod error {
    pub(crate) use crate::vendormod::error::*;
}

use crate::vendormod::{VendorMod, GO_CONFIG};
use buildsys::manifest;
use error::Result;
use filetime::FileTime;
use std::path::Path;

pub(crate) struct GoMod;

impl GoMod {
    pub(crate) fn vendor(
        root_dir: &Path,
        package_dir: &Path,
        external_file: &manifest::ExternalFile,
        sdk: &str,
        mtime: FileTime,
    ) -> Result<()> {
        VendorMod::vendor(&GO_CONFIG, root_dir, package_dir, external_file, sdk, mtime)
    }
}
