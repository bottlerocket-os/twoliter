/*!
`pubsys setup` helps you get started with the credentials you need to make Bottlerocket images and
the repos you use to update them.  Specifically, it can create a new key and role, or download an
existing role.
*/

#![deny(rust_2018_idioms)]

use log::{debug, info, trace, warn};
use pubsys_config::InfraConfig;
use sha2::{Digest, Sha512};
use simplelog::{Config as LogConfig, LevelFilter, TermLogger, TerminalMode};
use snafu::{ensure, OptionExt, ResultExt};
use std::convert::TryFrom;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{self, Command};
use structopt::StructOpt;
use tempfile::NamedTempFile;
use url::Url;

/// Helps you get started with credentials to make Bottlerocket images and repos.
#[derive(Debug, StructOpt)]
struct Args {
    #[structopt(global = true, long, default_value = "INFO")]
    /// How much detail to log; from least to most: ERROR, WARN, INFO, DEBUG, TRACE
    log_level: LevelFilter,

    #[structopt(long, parse(from_os_str))]
    /// Path to Infra.toml
    infra_config_path: PathBuf,

    #[structopt(long)]
    /// Use this named repo from Infra.toml
    repo: String,

    #[structopt(long, parse(from_os_str))]
    /// Path to root.json
    root_role_path: PathBuf,
    #[structopt(long, parse(from_os_str))]
    /// If we have to generate a local key, store it here
    default_key_path: PathBuf,

    #[structopt(long)]
    /// Allow setup to continue if we have a root role but no key for it
    allow_missing_key: bool,
}

/// The tuftool macro wraps Command to simplify calls to tuftool.
macro_rules! tuftool {
    // We use variadic arguments to wrap a format! call so the user doesn't need to call format!
    // each time.  `tuftool root` always requires the path to root.json so there's always at least
    // one.
    ($format_str:expr, $($format_arg:expr),*) => {
        let arg_str = format!($format_str, $($format_arg),*);
        trace!("tuftool arg string: {}", arg_str);
        let args = shell_words::split(&arg_str).context(error::CommandSplit { command: &arg_str })?;
        trace!("tuftool split args: {:#?}", args);

        let status = Command::new("tuftool")
            .args(args)
            .status()
            .context(error::TuftoolSpawn)?;

        ensure!(status.success(), error::TuftoolResult {
            command: arg_str,
            code: status.code().map(|i| i.to_string()).unwrap_or_else(|| "<unknown>".to_string())
        });
    }
}

/// Main entry point for tuftool setup.
fn run() -> Result<()> {
    // Parse and store the args passed to the program
    let args = Args::from_args();

    // TerminalMode::Mixed will send errors to stderr and anything less to stdout.
    TermLogger::init(args.log_level, LogConfig::default(), TerminalMode::Mixed)
        .context(error::Logger)?;

    // Make /roles and /keys directories, if they don't exist, so we can write generated files.
    let role_dir = args.root_role_path.parent().context(error::Path {
        path: &args.root_role_path,
        thing: "root role",
    })?;
    let key_dir = args.default_key_path.parent().context(error::Path {
        path: &args.default_key_path,
        thing: "key",
    })?;
    fs::create_dir_all(role_dir).context(error::Mkdir { path: role_dir })?;
    fs::create_dir_all(key_dir).context(error::Mkdir { path: key_dir })?;

    // Main branching logic for deciding whether to create role/key, use what we have, or error.
    match find_root_role_and_key(&args)? {
        (Some(_root_role_path), Some(_key_url)) => Ok(()),
        (Some(_root_role_path), None) => {
            ensure!(
                args.allow_missing_key,
                error::MissingKey { repo: args.repo }
            );
            Ok(())
        }
        // User is missing something, so we generate at least a root.json and maybe a key.
        (None, maybe_key_url) => {
            if maybe_key_url.is_some() {
                info!("Didn't find root role in Infra.toml, generating...");
            } else {
                info!("Didn't find root role or signing key in Infra.toml, generating...");
            }

            let temp_root_role =
                NamedTempFile::new_in(&role_dir).context(error::TempFileCreate {
                    purpose: "root role",
                })?;
            let temp_root_role_path = temp_root_role.path().display();

            // Make tuftool calls to create an initial root.json with basic parameters.
            tuftool!("root init '{}'", temp_root_role_path);

            tuftool!("root expire '{}' 'in 52 weeks'", temp_root_role_path);

            tuftool!("root set-threshold '{}' root 1", temp_root_role_path);
            tuftool!("root set-threshold '{}' snapshot 1", temp_root_role_path);
            tuftool!("root set-threshold '{}' targets 1", temp_root_role_path);
            tuftool!("root set-threshold '{}' timestamp 1", temp_root_role_path);

            let key_url = if let Some(key_url) = maybe_key_url {
                // If the user has a key, add it to each role.
                tuftool!("root add-key '{}' '{}' --role root --role snapshot --role targets --role timestamp",
                         temp_root_role_path, key_url);
                key_url
            } else {
                // If the user has no key, build one and add it to each role.
                tuftool!("root gen-rsa-key '{}' '{}' --role root --role snapshot --role targets --role timestamp",
                         temp_root_role_path, args.default_key_path.display());
                warn!(
                    "Created a key at {} - note that for production use, you should \
                     use a key stored in a trusted service like KMS or SSM",
                    args.default_key_path.display()
                );

                Url::from_file_path(&args.default_key_path)
                    .ok()
                    .context(error::FileUrl {
                        path: args.default_key_path,
                    })?
            };

            // Sign the role with the given key.
            tuftool!("root sign '{}' -k '{}'", temp_root_role_path, key_url);

            temp_root_role
                .persist_noclobber(&args.root_role_path)
                .context(error::TempFilePersist {
                    path: &args.root_role_path,
                })?;

            warn!(
                "Created a root role at {} - note that for production use, you should create \
                    a role with a shorter expiration and higher thresholds",
                args.root_role_path.display()
            );

            // Root role files don't need to be secret.
            fs::set_permissions(&args.root_role_path, fs::Permissions::from_mode(0o644)).context(
                error::SetMode {
                    path: &args.root_role_path,
                },
            )?;

            Ok(())
        }
    }
}

/// Searches Infra.toml and expected local paths for a root role and key for the requested repo.
fn find_root_role_and_key(args: &Args) -> Result<(Option<&PathBuf>, Option<Url>)> {
    let (mut root_role_path, mut key_url) = (None, None);

    if args.infra_config_path.exists() {
        info!(
            "Found infra config at path: {}",
            args.infra_config_path.display()
        );

        let infra_config =
            InfraConfig::from_path(&args.infra_config_path).context(error::Config)?;
        trace!("Parsed infra config: {:?}", infra_config);

        // Check whether the user has the relevant repo defined in their Infra.toml.
        if let Some(repo_config) = infra_config
            .repo
            .as_ref()
            .and_then(|repo_section| repo_section.get(&args.repo))
        {
            // If they have a root role URL and checksum defined, we can download it.
            if let (Some(url), Some(sha512)) =
                (&repo_config.root_role_url, &repo_config.root_role_sha512)
            {
                // If it's already been downloaded, just confirm the checksum.
                if args.root_role_path.exists() {
                    let root_role_data =
                        fs::read_to_string(&args.root_role_path).context(error::ReadFile {
                            path: &args.root_role_path,
                        })?;
                    let mut d = Sha512::new();
                    d.update(&root_role_data);
                    let digest = hex::encode(d.finalize());

                    ensure!(
                        &digest == sha512,
                        error::Hash {
                            expected: sha512,
                            got: digest,
                            thing: args.root_role_path.to_string_lossy()
                        }
                    );
                    debug!(
                        "Using existing downloaded root role at {}",
                        args.root_role_path.display()
                    );
                } else {
                    // Download the root role by URL and verify its checksum before writing it.
                    let root_role_data = reqwest::blocking::get(url.clone())
                        .with_context(|| error::GetUrl { url: url.clone() })?
                        .text()
                        .with_context(|| error::GetUrl { url: url.clone() })?;

                    let mut d = Sha512::new();
                    d.update(&root_role_data);
                    let digest = hex::encode(d.finalize());

                    ensure!(
                        &digest == sha512,
                        error::Hash {
                            expected: sha512,
                            got: digest,
                            thing: url.to_string()
                        }
                    );

                    // Write root role to expected path on disk.
                    fs::write(&args.root_role_path, &root_role_data).context(error::WriteFile {
                        path: &args.root_role_path,
                    })?;
                    debug!("Downloaded root role to {}", args.root_role_path.display());
                }

                root_role_path = Some(&args.root_role_path);
            } else if repo_config.root_role_url.is_some() || repo_config.root_role_sha512.is_some()
            {
                // Must specify both URL and checksum.
                error::RootRoleConfig.fail()?;
            }

            if let Some(key_config) = &repo_config.signing_keys {
                key_url = Some(
                    Url::try_from(key_config.clone())
                        .ok()
                        .context(error::SigningKeyUrl { repo: &args.repo })?,
                );
            }
        } else {
            info!(
                "No repo config in '{}' - using local roles/keys",
                args.infra_config_path.display()
            );
        }
    } else {
        info!(
            "No infra config at '{}' - using local roles/keys",
            args.infra_config_path.display()
        );
    }

    // If they don't have an Infra.toml or didn't define a root role / key there, check for them in
    // expected local paths.
    if root_role_path.is_none() && args.root_role_path.exists() {
        root_role_path = Some(&args.root_role_path);
    }
    if key_url.is_none() && args.default_key_path.exists() {
        key_url = Some(Url::from_file_path(&args.default_key_path).ok().context(
            error::FileUrl {
                path: &args.default_key_path,
            },
        )?);
    }

    Ok((root_role_path, key_url))
}

// Returning a Result from main makes it print a Debug representation of the error, but with Snafu
// we have nice Display representations of the error, so we wrap "main" (run) and print any error.
// https://github.com/shepmaster/snafu/issues/110
fn main() {
    if let Err(e) = run() {
        eprintln!("{}", e);
        process::exit(1);
    }
}

mod error {
    use snafu::Snafu;
    use std::io;
    use std::path::PathBuf;
    use url::Url;

    #[derive(Debug, Snafu)]
    #[snafu(visibility = "pub(super)")]
    pub(super) enum Error {
        #[snafu(display("Error splitting shell command - {} - input: {}", source, command))]
        CommandSplit {
            command: String,
            source: shell_words::ParseError,
        },

        #[snafu(display("Error reading config: {}", source))]
        Config { source: pubsys_config::Error },

        #[snafu(display("Path not valid as a URL: {}", path.display()))]
        FileUrl { path: PathBuf },

        #[snafu(display("Failed to fetch URL '{}': {}", url, source))]
        GetUrl { url: Url, source: reqwest::Error },

        #[snafu(display("Hash mismatch for '{}', got {} but expected {}", thing, got, expected))]
        Hash {
            expected: String,
            got: String,
            thing: String,
        },

        #[snafu(display("Logger setup error: {}", source))]
        Logger { source: simplelog::TermLogError },

        #[snafu(display("'{}' repo has root role but no key.  You wouldn't be able to update a repo without the matching key.  To continue, pass '-e ALLOW_MISSING_KEY=true'", repo))]
        MissingKey { repo: String },

        #[snafu(display("Failed to create '{}': {}", path.display(), source))]
        Mkdir { path: PathBuf, source: io::Error },

        #[snafu(display("Invalid path '{}' for {}", path.display(), thing))]
        Path { path: PathBuf, thing: String },

        #[snafu(display("Failed to read '{}': {}", path.display(), source))]
        ReadFile { path: PathBuf, source: io::Error },

        #[snafu(display(
            "Must specify both URL and SHA512 of root role in Infra.toml, found only one"
        ))]
        RootRoleConfig,

        #[snafu(display("Failed to set permissions on {}: {}", path.display(), source))]
        SetMode { path: PathBuf, source: io::Error },

        #[snafu(display("Failed to create temp file for {}: {}", purpose, source))]
        TempFileCreate { purpose: String, source: io::Error },

        #[snafu(display("Failed to move temp file to {}: {}", path.display(), source))]
        TempFilePersist {
            path: PathBuf,
            source: tempfile::PersistError,
        },

        #[snafu(display("Returned {}: tuftool {}", code, command))]
        TuftoolResult { code: String, command: String },

        #[snafu(display("Failed to start tuftool: {}", source))]
        TuftoolSpawn { source: io::Error },

        #[snafu(display("Unable to build URL from signing key for repo '{}'", repo))]
        SigningKeyUrl { repo: String },

        #[snafu(display("Failed to write '{}': {}", path.display(), source))]
        WriteFile { path: PathBuf, source: io::Error },
    }
}
type Result<T> = std::result::Result<T, error::Error>;
