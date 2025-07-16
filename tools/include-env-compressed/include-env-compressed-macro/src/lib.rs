extern crate proc_macro;

use proc_macro2::Literal;
use proc_macro2::TokenStream;
use quote::quote;
use snafu::{Report, ResultExt, Snafu};
use syn::{LitInt, LitStr, Token, parse::Parse, parse_macro_input};

struct Zstd;
struct Uncompressed;

#[cfg(debug_assertions)]
type CompressionProfile = Uncompressed;
#[cfg(not(debug_assertions))]
type CompressionProfile = Zstd;

#[proc_macro]
/// Include bytes of a file based on the value of an environment variable.
///
/// A zstd compression level may optionally be passed as a second argument.
/// This returns an `include_env_compressed::Archive` containing the resulting bytes.
/// For debug builds, the included bytes remain uncompressed.
///
/// ```ignore
/// use include_env_compressed;
/// const MY_ARCHIVE: Archive = include_archive_from_env!("CARGO_BIN_FILE_MYBINARY");
///
/// let byte_reader = MY_ARCHIVE.reader(); // Returns a `Box<dyn Read + Send + Sync + 'static>`
/// ```
pub fn include_archive_from_env(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as MacroArgs<CompressionProfile>);
    _include_archive_from_env(input)
}

#[proc_macro]
/// Include bytes of a file based on the value of an environment variable.
///
/// A zstd compression level may optionally be passed as a second argument.
/// This returns an `include_env_compressed::Archive` containing the resulting zstd-comrpessed
/// bytes.
///
/// ```ignore
/// use include_env_compressed_macro;
/// const MY_ARCHIVE: Archive = include_archive_from_env!("CARGO_BIN_FILE_MYBINARY");
///
/// let byte_reader = MY_ARCHIVE.reader(); // Returns a `Box<dyn Read + Send + Sync + 'static>`
/// ```
pub fn include_zstd_archive_from_env(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as MacroArgs<Zstd>);
    _include_archive_from_env(input)
}

#[proc_macro]
/// Include bytes of a file based on the value of an environment variable.
///
/// This returns an `include_env_compressed::Archive` containing the resulting uncompressed bytes.
///
/// ```ignore
/// use include_env_compressed_macro;
/// const MY_ARCHIVE: Archive = include_archive_from_env!("CARGO_BIN_FILE_MYBINARY");
///
/// let byte_reader = MY_ARCHIVE.reader(); // Returns a `Box<dyn Read + Send + Sync + 'static>`
/// ```
pub fn include_uncompressed_archive_from_env(
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as MacroArgs<Uncompressed>);
    _include_archive_from_env(input)
}

fn _include_archive_from_env(archive: impl IncludedArchive) -> proc_macro::TokenStream {
    archive
        .emit_archive()
        .unwrap_or_else(|e| {
            let err_msg = Report::from_error(e).to_string();
            quote! {
                compile_error!(#err_msg);
            }
        })
        .into()
}

#[derive(Debug)]
struct MacroArgs<Compression> {
    env_var: String,
    level: i32,
    _compression: std::marker::PhantomData<Compression>,
}

trait IncludedArchive {
    fn emit_archive(&self) -> Result<TokenStream, IncludeError>;
}

impl IncludedArchive for MacroArgs<Zstd> {
    fn emit_archive(&self) -> Result<TokenStream, IncludeError> {
        let env_var = &self.env_var;
        let path = std::env::var(env_var).context(EnvVarSnafu { env_var })?;

        let data = std::fs::read(&path).context(ReadArchiveSnafu { path })?;

        let compressed = zstd::encode_all(data.as_slice(), self.level).unwrap();
        let literal = Literal::byte_string(&compressed);

        Ok(quote! {
            ::include_env_compressed::Archive::zstd(#literal)
        })
    }
}

impl<Compression> Parse for MacroArgs<Compression> {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let env_var = input.parse::<LitStr>()?.value();

        let level = input
            .peek(Token![,])
            .then(|| {
                let _comma: Token![,] = input.parse()?;
                input.parse::<LitInt>()?.base10_parse::<i32>()
            })
            .transpose()?
            .unwrap_or_default();

        Ok(MacroArgs {
            env_var,
            level,
            _compression: std::marker::PhantomData,
        })
    }
}

impl IncludedArchive for MacroArgs<Uncompressed> {
    fn emit_archive(&self) -> Result<TokenStream, IncludeError> {
        let env_var = &self.env_var;
        Ok(quote! {
            ::include_env_compressed::Archive::uncompressed(include_bytes!(env!(#env_var)))
        })
    }
}

#[derive(Debug, Snafu)]
enum IncludeError {
    #[snafu(display("Could not determine archive path from env var '{env_var}'",))]
    EnvVar {
        env_var: String,
        source: std::env::VarError,
    },

    #[snafu(display("Failed to read file '{path}'"))]
    ReadArchive {
        path: String,
        source: std::io::Error,
    },
}
