use coldsnap::{SnapshotUploader, UploadZeroBlocks};
use indicatif::{ProgressBar, ProgressStyle};
use snafu::{OptionExt, ResultExt};
use std::path::Path;

/// Create a progress bar to show status of snapshot blocks.
pub(crate) fn build_progress_bar(verb: &str) -> Result<ProgressBar> {
    let progress_bar = ProgressBar::new(0);
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template(&["  ", verb, "  [{bar:50.white/black}] {pos}/{len} ({eta})"].concat())
            .context(error::ProgressBarTemplateSnafu)?
            .progress_chars("=> "),
    );
    Ok(progress_bar)
}

/// Uploads the given path into a snapshot.
pub(crate) async fn snapshot_from_image<P>(
    path: P,
    uploader: &SnapshotUploader,
    desired_size: Option<i64>,
    progress_bar: Option<ProgressBar>,
) -> Result<String>
where
    P: AsRef<Path>,
{
    let path = path.as_ref();
    let filename = path
        .file_name()
        .context(error::InvalidImagePathSnafu { path })?
        .to_string_lossy();

    uploader
        .upload_from_file(
            path,
            desired_size,
            Some(&filename),
            None,
            progress_bar.clone(),
            Some(UploadZeroBlocks::Omit),
            None,
        )
        .await
        .context(error::UploadSnapshotSnafu)
}

mod error {
    use snafu::Snafu;
    use std::path::PathBuf;

    #[derive(Debug, Snafu)]
    #[snafu(visibility(pub(super)))]
    #[allow(clippy::large_enum_variant)]
    pub(crate) enum Error {
        #[snafu(display("Invalid image path '{}'", path.display()))]
        InvalidImagePath { path: PathBuf },

        #[snafu(display("Failed to parse progress style template: {}", source))]
        ProgressBarTemplate {
            source: indicatif::style::TemplateError,
        },

        #[snafu(display("Failed to upload snapshot: {}", source))]
        UploadSnapshot {
            #[snafu(source(from(coldsnap::UploadError, Box::new)))]
            source: Box<coldsnap::UploadError>,
        },
    }
}
pub(crate) use error::Error;
type Result<T> = std::result::Result<T, error::Error>;
