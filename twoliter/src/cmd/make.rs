use crate::cargo_make::CargoMake;
use crate::project::{self, Locked, SDKLocked, Unlocked};
use crate::tools::install_tools;
use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

// Most subcommands do not require kits and thus do not need to resolve and verify them against the
// lockfile.
//
// Avoiding that resolution can be useful in CI situations where images are already built and we
// need to perform additional operations using the SDK.
//
// Only twoliter make targets in the following list *require* kit validation; however, kit
// validation will occur in other targets which require the SDK if no SDK is explicitly listed in
// Twoliter.toml.
const MUST_VALIDATE_KITS_TARGETS: &[&str] = &[
    "build-package",
    "build-kit",
    "build-variant",
    "build-all",
    "build",
    "default",
];

/// Run a cargo make command in Twoliter's build environment. Known Makefile.toml environment
/// variables will be passed-through to the cargo make invocation.
#[derive(Debug, Parser)]
#[clap(trailing_var_arg = true)]
pub(crate) struct Make {
    /// Path to the project file. Will search for Twoliter.toml when absent.
    #[clap(long)]
    project_path: Option<PathBuf>,

    /// Twoliter does not read this from the CARGO_HOME environment variable to avoid any possible
    /// confusion between a CARGO_HOME set on the system, and the path intended for the Bottlerocket
    /// build.
    #[clap(long)]
    cargo_home: PathBuf,

    /// This can be passed by environment variable. We require it as part of the command arguments
    /// because we need it to pull the right SDK target architecture.
    #[clap(long, env = "BUILDSYS_ARCH")]
    arch: String,

    /// Cargo make task. E.g. the word "build" if we want to execute `cargo make build`.
    makefile_task: String,

    /// Uninspected arguments to be passed to cargo make after the target name. For example, --foo
    /// in the following command : cargo make test --foo.
    additional_args: Vec<String>,
}

impl Make {
    pub(super) async fn run(&self) -> Result<()> {
        let project = project::load_or_find_project(self.project_path.clone()).await?;
        let sdk_source = self.locked_sdk(&project).await?;
        let toolsdir = project.project_dir().join("build/tools");
        install_tools(&toolsdir).await?;
        let makefile_path = toolsdir.join("Makefile.toml");
        CargoMake::new(&sdk_source)?
            .env("CARGO_HOME", self.cargo_home.display().to_string())
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .env("BUILDSYS_VERSION_IMAGE", project.release_version())
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec_with_args(&self.makefile_task, self.additional_args.clone())
            .await
    }

    fn can_skip_kit_verification(&self, project: &project::Project<Unlocked>) -> bool {
        let target_allows_kit_verification_skip =
            !MUST_VALIDATE_KITS_TARGETS.contains(&self.makefile_task.as_str());
        let project_has_explicit_sdk_dep = project.direct_sdk_image_dep().is_some();

        target_allows_kit_verification_skip && project_has_explicit_sdk_dep
    }

    /// Returns the locked SDK image for the project.
    async fn locked_sdk(&self, project: &project::Project<Unlocked>) -> Result<String> {
        Ok(if self.can_skip_kit_verification(project) {
            project.load_lock::<SDKLocked>().await?.sdk_image()
        } else {
            project.load_lock::<Locked>().await?.sdk_image()
        }
        .project_image_uri()
        .to_string())
    }
}

#[cfg(test)]
mod test {
    use std::path::Path;

    use crate::cmd::update::Update;
    use crate::project::VerificationTagger;

    use super::*;

    #[test]
    fn test_trailing_args_1() {
        let args = Make::try_parse_from([
            "make",
            "--cargo-home",
            "/tmp/foo",
            "--arch",
            "x86_64",
            "testsys",
            "--",
            "add",
            "secret",
            "map",
            "--name",
            "foo",
            "something=bar",
            "something-else=baz",
        ])
        .unwrap();

        assert_eq!(args.makefile_task, "testsys");
        assert_eq!(args.additional_args[0], "add");
        assert_eq!(args.additional_args[1], "secret");
        assert_eq!(args.additional_args[2], "map");
        assert_eq!(args.additional_args[3], "--name");
        assert_eq!(args.additional_args[4], "foo");
        assert_eq!(args.additional_args[5], "something=bar");
        assert_eq!(args.additional_args[6], "something-else=baz");
    }

    #[test]
    fn test_trailing_args_2() {
        let args = Make::try_parse_from([
            "make",
            "--cargo-home",
            "/tmp/foo",
            "--arch",
            "x86_64",
            "testsys",
            "add",
            "secret",
            "map",
            "--name",
            "foo",
            "something=bar",
            "something-else=baz",
        ])
        .unwrap();

        assert_eq!(args.makefile_task, "testsys");
        assert_eq!(args.additional_args[0], "add");
        assert_eq!(args.additional_args[1], "secret");
        assert_eq!(args.additional_args[2], "map");
        assert_eq!(args.additional_args[3], "--name");
        assert_eq!(args.additional_args[4], "foo");
        assert_eq!(args.additional_args[5], "something=bar");
        assert_eq!(args.additional_args[6], "something-else=baz");
    }

    #[test]
    fn test_trailing_args_3() {
        let args = Make::try_parse_from([
            "make",
            "--cargo-home",
            "/tmp/foo",
            "--arch",
            "x86_64",
            "testsys",
            "--",
            "add",
            "secret",
            "map",
            "--",
            "--name",
            "foo",
            "something=bar",
            "something-else=baz",
            "--",
        ])
        .unwrap();

        assert_eq!(args.makefile_task, "testsys");
        assert_eq!(args.additional_args[0], "add");
        assert_eq!(args.additional_args[1], "secret");
        assert_eq!(args.additional_args[2], "map");
        // The first instance of `--`, between `testsys` and `add`, is not passed through to the
        // varargs. After that, instances of `--` are passed through the varargs.
        assert_eq!(args.additional_args[3], "--");
        assert_eq!(args.additional_args[4], "--name");
        assert_eq!(args.additional_args[5], "foo");
        assert_eq!(args.additional_args[6], "something=bar");
        assert_eq!(args.additional_args[7], "something-else=baz");
        assert_eq!(args.additional_args[8], "--");
    }

    const PROJECT: &str = "local-kit";

    async fn twoliter_update(project_path: &Path) {
        let command = Update {
            project_path: Some(project_path.to_path_buf()),
        };
        command.run().await.unwrap();
    }

    async fn run_makefile_target(
        target: &str,
        project_dir: &Path,
        delete_verifier_tags: bool,
    ) -> Result<()> {
        let project_path = project_dir.join("Twoliter.toml");

        twoliter_update(&project_path).await;

        let project = project::load_or_find_project(Some(project_path))
            .await
            .unwrap();
        let project = project.load_lock::<SDKLocked>().await.unwrap();
        let sdk_source = project.sdk_image().project_image_uri().to_string();

        if delete_verifier_tags {
            // Clean up tags so that the build fails
            VerificationTagger::cleanup_existing_tags(project.external_kits_dir())
                .await
                .unwrap();
        }

        let toolsdir = project.project_dir().join("build/tools");
        install_tools(&toolsdir).await.unwrap();
        let makefile_path = toolsdir.join("Makefile.toml");

        CargoMake::new(&sdk_source)
            .unwrap()
            .env("CARGO_HOME", project_dir.display().to_string())
            .env("TWOLITER_TOOLS_DIR", toolsdir.display().to_string())
            .env("BUILDSYS_VERSION_IMAGE", project.release_version())
            .makefile(makefile_path)
            .project_dir(project.project_dir())
            .exec_with_args(target, Vec::<&'static str>::new())
            .await
    }

    async fn target_can_skip_kit_verification(target_name: &str) -> bool {
        let temp_dir = crate::test::copy_project_to_temp_dir(PROJECT);
        let project_dir = temp_dir.path();
        let project_path = project_dir.join("Twoliter.toml");
        let project = project::load_or_find_project(Some(project_path.clone()))
            .await
            .unwrap();

        let make = Make {
            project_path: Some(project_path),
            cargo_home: project_dir.to_owned(),
            arch: "x86_64".to_string(),
            makefile_task: target_name.to_string(),
            additional_args: Vec::new(),
        };
        make.can_skip_kit_verification(&project)
    }

    #[tokio::test]
    async fn test_repack_variant_can_skip_kit_verification() {
        assert!(target_can_skip_kit_verification("repack-variant").await);
    }

    #[tokio::test]
    async fn test_build_variant_cannot_skip_kit_verification() {
        assert!(!target_can_skip_kit_verification("build-variant").await);
    }

    #[tokio::test]
    #[ignore] // integration test
    async fn test_fetch_sdk_succeeds_when_only_sdk_verified() {
        let temp_dir = crate::test::copy_project_to_temp_dir(PROJECT);
        assert!(run_makefile_target("fetch-sdk", &temp_dir.path(), false)
            .await
            .is_ok());
    }

    #[tokio::test]
    #[ignore] // integration test
    async fn test_fetch_sdk_fails_when_nothing_verified() {
        let temp_dir = crate::test::copy_project_to_temp_dir(PROJECT);
        assert!(run_makefile_target("fetch-sdk", &temp_dir.path(), true)
            .await
            .is_err());
    }

    #[tokio::test]
    #[ignore] // integration test
    async fn test_validate_kits_fails_when_only_sdk_verified() {
        let temp_dir = crate::test::copy_project_to_temp_dir(PROJECT);
        assert!(
            run_makefile_target("validate-kits", &temp_dir.path(), false)
                .await
                .is_err()
        );
    }
}
