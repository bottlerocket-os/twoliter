use crate::bootconfig::syntax;
use crate::grub::config;
use snafu::Snafu;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub(crate)))]
pub enum Error {
    #[snafu(display("Bootconfig data too short: {length} bytes"))]
    BinaryTooShort { length: usize },

    #[snafu(display("Invalid bootconfig magic"))]
    InvalidMagic,

    #[snafu(display("Bootconfig size {size} exceeds data length {length}"))]
    InvalidBinarySize { size: usize, length: usize },

    #[snafu(display("Bootconfig checksum mismatch: expected {expected}, calculated {actual}"))]
    ChecksumMismatch { expected: u32, actual: u32 },

    #[snafu(display("Invalid UTF-8 in {input}: {source}"))]
    InvalidUtf8 {
        input: &'static str,
        source: std::str::Utf8Error,
    },

    #[snafu(display("Failed to parse bootconfig: {source}"))]
    BootconfigParse {
        #[snafu(source(from(pest::error::Error<syntax::Rule>, Box::new)))]
        source: Box<pest::error::Error<syntax::Rule>>,
    },

    #[snafu(display("Unsupported quote in '{name}'"))]
    UnsupportedQuote { name: String },

    #[snafu(display("Failed to parse grub.cfg: {source}"))]
    GrubParse {
        #[snafu(source(from(pest::error::Error<config::Rule>, Box::new)))]
        source: Box<pest::error::Error<config::Rule>>,
    },

    #[snafu(display("grub.cfg has no linux directive to boot"))]
    MissingGrubLinuxDirective,

    #[snafu(display("grub.cfg linux directive names no kernel"))]
    MissingGrubKernelPath,

    #[snafu(display("grub.cfg kernel path must be '{expected}', got: '{actual}'"))]
    InvalidGrubKernelPath {
        expected: &'static str,
        actual: String,
    },

    #[snafu(display("UKI command line must be printable ASCII"))]
    UnprintableCharacter,

    #[snafu(display("UKI command line must separate its arguments with single spaces"))]
    UncollapsedWhitespace,

    #[snafu(display("UKI command line has an unpaired quote"))]
    UnpairedQuote,

    #[snafu(display("UKI command line has data after its terminating NUL"))]
    DataAfterNul,
}
