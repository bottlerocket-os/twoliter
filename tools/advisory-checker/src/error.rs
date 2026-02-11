use snafu::Snafu;
use std::path::PathBuf;
use std::process::Output;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub), module)]
pub enum Error {
    #[snafu(display("Failed to execute command: {}", source))]
    ExecutionFailure { source: std::io::Error },

    #[snafu(display("Command '{}' failed: {}", bin_path, String::from_utf8_lossy(&output.stderr)))]
    CommandFailure { bin_path: String, output: Output },

    #[snafu(display("Failed to read '{}'", path.display()))]
    ReadFile {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to parse advisory '{}'", path.display()))]
    ParseAdvisory {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[snafu(display(
        "Incorrect output format for rpmspec command. Expected name|epoch|version. Got '{spec_output}'"
    ))]
    RpmSpecFormat { spec_output: String },

    #[snafu(display("Failed to read directory '{}'", path.display()))]
    ReadDir {
        path: PathBuf,
        source: std::io::Error,
    },

    #[snafu(display("Failed to read directory entry"))]
    ReadDirEntry { source: std::io::Error },

    #[snafu(display("{} Advisory violations found.", violations.len()))]
    AdvisoryViolations { violations: Vec<String> },
}
