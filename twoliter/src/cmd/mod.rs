mod build;
mod build_clean;
mod debug;
mod fetch;
mod make;
mod update;

use self::build::BuildCommand;
use crate::cmd::debug::DebugAction;
use crate::cmd::fetch::Fetch;
use crate::cmd::make::Make;
use crate::cmd::update::Update;
use anyhow::Result;
use clap::Parser;
use env_logger::Builder;
use log::LevelFilter;

const DEFAULT_LEVEL_FILTER: LevelFilter = LevelFilter::Info;

/// A tool for building custom variants of Bottlerocket.
#[derive(Debug, Parser)]
#[clap(about, long_about = None, version)]
pub(crate) struct Args {
    /// Set the logging level. One of [off|error|warn|info|debug|trace]. Defaults to warn. You can
    /// also leave this unset and use the RUST_LOG env variable. See
    /// https://github.com/rust-cli/env_logger/
    #[clap(long = "log-level")]
    pub(crate) log_level: Option<LevelFilter>,

    #[clap(subcommand)]
    pub(crate) subcommand: Subcommand,
}

#[derive(Debug, Parser)]
pub(crate) enum Subcommand {
    /// Build something, such as a Bottlerocket image or a kit of packages.
    #[clap(subcommand)]
    Build(BuildCommand),

    Fetch(Fetch),

    Make(Make),

    /// Update Twoliter.lock
    Update(Update),

    /// Commands that are used for checking and troubleshooting Twoliter's internals.
    #[clap(subcommand)]
    Debug(DebugAction),
}

/// Entrypoint for the `twoliter` command line program.
pub(super) async fn run(args: Args) -> Result<()> {
    match args.subcommand {
        Subcommand::Build(build_command) => build_command.run().await,
        Subcommand::Fetch(fetch_args) => fetch_args.run().await,
        Subcommand::Make(make_args) => make_args.run().await,
        Subcommand::Update(update_args) => update_args.run().await,
        Subcommand::Debug(debug_action) => debug_action.run().await,
    }
}

/// use `level` if present, or else use `RUST_LOG` if present, or else use a default.
pub(super) fn init_logger(level: Option<LevelFilter>) {
    match (std::env::var(env_logger::DEFAULT_FILTER_ENV).ok(), level) {
        (Some(_), None) => {
            // RUST_LOG exists and level does not; use the environment variable.
            Builder::from_default_env().init();
        }
        _ => {
            // use provided log level or default for this crate only.
            Builder::new()
                .filter(
                    Some(env!("CARGO_CRATE_NAME")),
                    level.unwrap_or(DEFAULT_LEVEL_FILTER),
                )
                .init();
        }
    }
}

#[cfg(feature = "integ-tests")]
#[cfg(test)]
mod test {
    use super::*;
    use crate::cmd::build::BuildKit;
    use std::path::Path;

    const PROJECT: &str = "local-kit";

    fn expect_kit(project_dir: &Path, name: &str, arch: &str, packages: &[&str]) {
        let build = project_dir.join("build");
        let kit_output_dir = build.join("kits").join(name).join(arch).join("Packages");
        assert!(
            kit_output_dir.is_dir(),
            "Expected to find output dir for {} at {}",
            name,
            kit_output_dir.display()
        );

        for package in packages {
            let rpm = kit_output_dir.join(&format!(
                "bottlerocket-{package}-0.0-00000000000.00000000.br1.{arch}.rpm"
            ));
            assert!(
                rpm.is_file(),
                "Expected to find RPM for {}, for {} at {}",
                package,
                name,
                rpm.display()
            );
        }
    }

    async fn twoliter_update(project_path: &Path) {
        let command = Update {
            project_path: Some(project_path.to_path_buf()),
        };
        command.run().await.unwrap();
    }

    async fn twoliter_fetch(project_path: &Path, arch: &str) {
        let command = Fetch {
            project_path: Some(project_path.to_path_buf()),
            arch: arch.into(),
        };
        command.run().await.unwrap()
    }

    #[tokio::test]
    async fn build_core_kit() {
        let kit_name = "core-kit";
        let arch = "aarch64";
        let temp_dir = crate::test::copy_project_to_temp_dir(PROJECT);
        let project_dir = temp_dir.path();
        let project_path = project_dir.join("Twoliter.toml");
        twoliter_update(&project_path).await;
        twoliter_fetch(&project_path, arch).await;

        let command = BuildKit {
            project_path: Some(project_path),
            arch: arch.to_string(),
            kit: kit_name.to_string(),
            lookaside_cache: None,
            upstream_source_fallback: false,
        };

        command.run().await.unwrap();
        expect_kit(&project_dir, "core-kit", arch, &["pkg-a"]);
    }

    #[tokio::test]
    async fn build_extra_1_kit() {
        let kit_name = "extra-1-kit";
        let arch = "x86_64";
        let temp_dir = crate::test::copy_project_to_temp_dir(PROJECT);
        let project_dir = temp_dir.path();
        let project_path = project_dir.join("Twoliter.toml");
        twoliter_update(&project_path).await;
        twoliter_fetch(&project_path, arch).await;

        let command = BuildKit {
            project_path: Some(project_path),
            arch: arch.to_string(),
            kit: kit_name.to_string(),
            lookaside_cache: None,
            upstream_source_fallback: false,
        };

        command.run().await.unwrap();
        expect_kit(&project_dir, "core-kit", arch, &["pkg-a"]);
        expect_kit(&project_dir, "extra-1-kit", arch, &["pkg-b", "pkg-d"]);
    }

    #[tokio::test]
    async fn build_extra_2_kit() {
        let kit_name = "extra-2-kit";
        let arch = "aarch64";
        let temp_dir = crate::test::copy_project_to_temp_dir(PROJECT);
        let project_dir = temp_dir.path();
        let project_path = project_dir.join("Twoliter.toml");
        twoliter_update(&project_path).await;
        twoliter_fetch(&project_path, arch).await;

        let command = BuildKit {
            project_path: Some(project_path),
            arch: arch.to_string(),
            kit: kit_name.to_string(),
            lookaside_cache: None,
            upstream_source_fallback: false,
        };

        command.run().await.unwrap();
        expect_kit(&project_dir, "core-kit", arch, &["pkg-a"]);
        expect_kit(&project_dir, "extra-2-kit", arch, &["pkg-c"]);
    }

    #[tokio::test]
    async fn build_extra_3_kit() {
        let kit_name = "extra-3-kit";
        let arch = "x86_64";
        let temp_dir = crate::test::copy_project_to_temp_dir(PROJECT);
        let project_dir = temp_dir.path();
        let project_path = project_dir.join("Twoliter.toml");
        twoliter_update(&project_path).await;
        twoliter_fetch(&project_path, arch).await;

        let command = BuildKit {
            project_path: Some(project_path),
            arch: arch.to_string(),
            kit: kit_name.to_string(),
            lookaside_cache: None,
            upstream_source_fallback: false,
        };

        command.run().await.unwrap();
        expect_kit(&project_dir, "core-kit", arch, &["pkg-a"]);
        expect_kit(&project_dir, "extra-1-kit", arch, &["pkg-b", "pkg-d"]);
        expect_kit(&project_dir, "extra-2-kit", arch, &["pkg-c"]);
        expect_kit(
            &project_dir,
            "extra-3-kit",
            arch,
            &["pkg-e", "pkg-f", "pkg-g"],
        );
    }
}
