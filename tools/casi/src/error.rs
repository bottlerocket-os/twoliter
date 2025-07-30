use snafu::Snafu;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display(
        "Failed to read data for hashing: {source} - check if the file exists and is readable"
    ))]
    HashingReadError { source: std::io::Error },

    #[snafu(display("Failed to initialize logging system: {source}"))]
    LogInit {
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[snafu(display("Failed to parse log directive: {directive}"))]
    LogDirectiveParse { directive: String },
}
