/*!
Many of the inputs to package builds are not source files tracked within the git
repository, but large binary artifacts such as tar archives that are independently
distributed by an upstream project.

This module provides the ability to retrieve and validate these external files,
given the (name, url, hash) data that uniquely identifies each file.

It implements a two-tier approach to retrieval: files are first pulled from the
"lookaside" cache and only fetched from the upstream site if that access fails.

*/
pub(crate) mod error;
use error::Result;

use buildsys::manifest;
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use sha2::{Digest, Sha512};
use snafu::{ensure, OptionExt, ResultExt};
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use url::Url;

pub(crate) struct LookasideCache {
    /// The version string to include in HTTP headers.
    version: String,

    /// The lookaside cache base URL for source tarballs.
    lookaside_cache: Url,

    /// Whether we are allowed to pull sources from upstream URLs. When this is false, it can be
    /// overridden by `upstream-fallback` in the manifest.
    upstream_fallback: bool,
}

impl LookasideCache {
    pub(crate) fn new(
        version: impl AsRef<str>,
        lookaside_cache: Url,
        upstream_fallback: bool,
    ) -> Self {
        Self {
            version: version.as_ref().to_string(),
            lookaside_cache,
            upstream_fallback,
        }
    }

    /// Fetch files stored out-of-tree and ensure they match the stored hash.
    pub(crate) fn fetch(&self, files: &[manifest::ExternalFile]) -> Result<()> {
        for f in files {
            let url_file_name = Self::extract_file_name(&f.url)?;
            let path = &f.path.as_ref().unwrap_or(&url_file_name);
            ensure!(
                path.components().count() == 1,
                error::ExternalFileNameSnafu { path }
            );

            let hash = &f.sha512;
            if path.is_file() {
                match Self::verify_file(path, hash) {
                    Ok(_) => continue,
                    Err(e) => {
                        println!("{}", e);
                        fs::remove_file(path).context(error::ExternalFileDeleteSnafu { path })?;
                    }
                }
            }

            let name = path.display();
            let tmp = PathBuf::from(format!(".{}", name));

            // first check the lookaside cache
            let url = format!("{}/{}/{}/{}", self.lookaside_cache, name, hash, name);
            match self.fetch_file(&url, &tmp, hash) {
                Ok(_) => {
                    fs::rename(&tmp, path)
                        .context(error::ExternalFileRenameSnafu { path: &tmp })?;
                    continue;
                }
                Err(e) => {
                    // next check with upstream, if permitted
                    if f.force_upstream.unwrap_or(false) || self.upstream_fallback {
                        println!("Error fetching from lookaside cache: {}", e);
                        println!("Fetching {:?} from upstream source", url_file_name);
                        self.fetch_file(&f.url, &tmp, hash)?;
                        fs::rename(&tmp, path)
                            .context(error::ExternalFileRenameSnafu { path: &tmp })?;
                    } else {
                        // we failed to fetch from the lookaside cache, and we cannot fall back to
                        // upstream sources, so we should not continue, we need to return the error
                        return Err(e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Retrieves a file from the specified URL and write it to the given path,
    /// then verifies the contents against the SHA-512 hash provided.
    fn fetch_file<P: AsRef<Path>>(&self, url: &str, path: P, hash: &str) -> Result<()> {
        let path = path.as_ref();

        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_str(&format!(
                "Bottlerocket buildsys {} (https://github.com/bottlerocket-os/bottlerocket)",
                self.version
            ))
            .unwrap_or(HeaderValue::from_static(
                "Bottlerocket buildsys (https://github.com/bottlerocket-os/bottlerocket)",
            )),
        );

        let client = reqwest::blocking::Client::new();
        let mut resp = client
            .get(url)
            .headers(headers)
            .send()
            .context(error::ExternalFileRequestSnafu { url })?;
        let status = resp.status();
        ensure!(
            status.is_success(),
            error::ExternalFileFetchSnafu { url, status }
        );

        let f = File::create(path).context(error::ExternalFileOpenSnafu { path })?;
        let mut f = BufWriter::new(f);
        resp.copy_to(&mut f)
            .context(error::ExternalFileSaveSnafu { path })?;
        drop(f);

        match Self::verify_file(path, hash) {
            Ok(_) => Ok(()),
            Err(e) => {
                fs::remove_file(path).context(error::ExternalFileDeleteSnafu { path })?;
                Err(e)
            }
        }
    }

    fn extract_file_name(url: &str) -> Result<PathBuf> {
        let parsed = reqwest::Url::parse(url).context(error::ExternalFileUrlSnafu { url })?;
        let name = parsed
            .path_segments()
            .context(error::ExternalFileNameSnafu { path: url })?
            .last()
            .context(error::ExternalFileNameSnafu { path: url })?;
        Ok(name.into())
    }

    /// Reads a file from disk and compares it to the expected SHA-512 hash.
    fn verify_file<P: AsRef<Path>>(path: P, hash: &str) -> Result<()> {
        let path = path.as_ref();
        let mut f = File::open(path).context(error::ExternalFileOpenSnafu { path })?;
        let mut d = Sha512::new();

        io::copy(&mut f, &mut d).context(error::ExternalFileLoadSnafu { path })?;
        let digest = hex::encode(d.finalize());

        ensure!(
            digest == hash,
            error::ExternalFileVerifySnafu { path, hash }
        );
        Ok(())
    }
}
