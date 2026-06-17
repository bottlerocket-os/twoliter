/*!
Shared vendoring abstraction for Go and Rust modules.

This module provides a unified implementation for vendoring dependencies
from upstream tar archives. Language-specific behavior is controlled via
`VendorConfig` structs.
*/

pub(crate) mod error;

use buildsys::manifest;
use duct::cmd;
use error::Result;
use filetime::{set_file_mtime, FileTime};
use snafu::{ensure, OptionExt, ResultExt};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::{env, fs};

/// Configuration for language-specific vendoring behavior.
pub(crate) struct VendorConfig {
    pub script_name: &'static str,
    pub script_template: &'static str,
    pub docker_tool: &'static str,
    pub cache_dir: &'static str,
    pub cache_arg_name: &'static str,
}

// The following bash template scripts are intended to be run within a container
// using the docker-go or docker-cargo tools found in this codebase.
//
// Each script inspects the top level directory found in the package upstream
// archive and uses that as the default module path if no explicit path was
// provided. It then untars the archive, vendors dependencies, creates a new
// archive of the vendor directory, and names it the output path provided. If no
// output path was given, it defaults to "bundled-{package-file-name}". Finally,
// it cleans up by removing the untar'd source code. The upstream archive
// remains intact and both tar files can then be used during packaging.
//
// These scripts exist as in-memory template string literals and are written to
// temporary files in the package directory itself to enable buildsys to be as
// portable as possible with no dependency on runtime paths. Since buildsys is
// executed from the context of many different package directories, managing a
// temporary file via this module prevents having to acquire the path of some
// static script file on the host system.

// Go vendoring configuration
const GO_SCRIPT_TMPL: &str = r#"#!/bin/bash

set -e

toplevel=$(tar tf "__LOCAL_FILE_NAME__" | head -1)
if [ -z "__MOD_DIR__" ] ; then
    targetdir="${toplevel}"
else
    targetdir="__MOD_DIR__"
fi

tar xf "__LOCAL_FILE_NAME__"

pushd "${targetdir}"
    go list -mod=readonly ./... >/dev/null && go mod vendor
popd

tar czf "__OUTPUT__" "${targetdir}"/vendor
rm -rf "${targetdir}"
touch -r "__LOCAL_FILE_NAME__" "__OUTPUT__"
"#;

pub(crate) const GO_CONFIG: VendorConfig = VendorConfig {
    script_name: "docker-go-script.sh",
    script_template: GO_SCRIPT_TMPL,
    docker_tool: "docker-go",
    cache_dir: ".gomodcache",
    cache_arg_name: "--go-mod-cache",
};

// Rust vendoring configuration
const RUST_SCRIPT_TMPL: &str = r#"#!/bin/bash

set -e

toplevel=$(tar tf "__LOCAL_FILE_NAME__" | head -1)
if [ -z "__MOD_DIR__" ] ; then
    targetdir="${toplevel}"
else
    targetdir="__MOD_DIR__"
fi

tar xf "__LOCAL_FILE_NAME__"

pushd "${targetdir}"
    mkdir -p .cargo
    cargo metadata --locked --format-version 1 >/dev/null && cargo vendor --locked > .cargo/config.toml
popd

tar czf "__OUTPUT__" -C "${targetdir}" vendor .cargo/config.toml
rm -rf "${targetdir}"
touch -r "__LOCAL_FILE_NAME__" "__OUTPUT__"
"#;

pub(crate) const RUST_CONFIG: VendorConfig = VendorConfig {
    script_name: "docker-cargo-script.sh",
    script_template: RUST_SCRIPT_TMPL,
    docker_tool: "docker-cargo",
    cache_dir: ".cargo",
    cache_arg_name: "--cargo-home",
};

pub(crate) struct VendorMod;

impl VendorMod {
    pub(crate) fn vendor(
        config: &VendorConfig,
        root_dir: &Path,
        package_dir: &Path,
        external_file: &manifest::ExternalFile,
        sdk: &str,
        mtime: FileTime,
    ) -> Result<()> {
        let url_file_name = extract_file_name(&external_file.url)?;
        let local_file_name = external_file.path.as_ref().unwrap_or(&url_file_name);
        ensure!(
            local_file_name.components().count() == 1,
            error::InputFileSnafu
        );

        let full_path = package_dir.join(local_file_name);
        ensure!(
            full_path.is_file(),
            error::InputFileBadSnafu { path: full_path }
        );

        // If a module directory was not provided, set as an empty path.
        // By default, without a provided module directory, tar will be passed
        // the first directory found in the archive as the top level module.
        let default_empty_path = PathBuf::from("");
        let mod_dir = external_file
            .bundle_root_path
            .as_ref()
            .unwrap_or(&default_empty_path);

        // Use a default "bundled-{name-of-file}" if no output path was provided.
        let default_output_path =
            PathBuf::from(format!("bundled-{}", local_file_name.to_string_lossy()));
        let output_path_arg = external_file
            .bundle_output_path
            .as_ref()
            .unwrap_or(&default_output_path);
        println!(
            "cargo:rerun-if-changed={}",
            output_path_arg.to_string_lossy()
        );

        // Create and/or write the temporary script file to the package directory
        // using the script template string and placeholder variables.
        let script_contents = config
            .script_template
            .replace("__LOCAL_FILE_NAME__", &local_file_name.to_string_lossy())
            .replace("__MOD_DIR__", &mod_dir.to_string_lossy())
            .replace("__OUTPUT__", &output_path_arg.to_string_lossy());
        let script_path = package_dir.join(config.script_name);

        // Drop the reference after writing the file to avoid a "text busy" error
        // when attempting to execute it.
        {
            let mut script_file = fs::File::create(&script_path)
                .context(error::CreateFileSnafu { path: &script_path })?;
            fs::set_permissions(&script_path, fs::Permissions::from_mode(0o777))
                .context(error::SetFilePermissionsSnafu { path: &script_path })?;
            script_file
                .write_all(script_contents.as_bytes())
                .context(error::WriteFileSnafu { path: &script_path })?;
        }

        let res = run_docker_tool(
            config,
            package_dir,
            sdk,
            &root_dir.join(config.cache_dir),
            &format!("./{}", config.script_name),
        );
        fs::remove_file(&script_path).context(error::RemoveFileSnafu { path: &script_path })?;

        if res.is_ok() {
            set_file_mtime(output_path_arg, mtime).context(error::SetMtimeSnafu {
                path: output_path_arg,
            })?;
        }

        res
    }
}

fn extract_file_name(url: &str) -> Result<PathBuf> {
    let parsed = reqwest::Url::parse(url).context(error::InputUrlSnafu { url })?;
    let name = parsed
        .path_segments()
        .context(error::InputFileBadSnafu { path: url })?
        .next_back()
        .context(error::InputFileBadSnafu { path: url })?;
    Ok(name.into())
}

fn run_docker_tool(
    config: &VendorConfig,
    module_path: &Path,
    sdk_image: &str,
    cache_dir: &Path,
    command: &str,
) -> Result<()> {
    let mut args = vec![
        "--module-path",
        module_path.to_str().context(error::InputFileSnafu)?,
        "--sdk-image",
        sdk_image,
        config.cache_arg_name,
        cache_dir.to_str().context(error::InputFileSnafu)?,
    ];

    args.push("--command");
    args.push(command);

    let arg_string = args.join(" ");
    let twoliter_tools_dir = env::var("TWOLITER_TOOLS_DIR").context(error::EnvironmentSnafu {
        var: "TWOLITER_TOOLS_DIR",
    })?;
    let program = PathBuf::from(twoliter_tools_dir).join(config.docker_tool);
    println!("program: {}", program.to_string_lossy());
    let output = cmd(program, args)
        .stderr_to_stdout()
        .stdout_capture()
        .unchecked()
        .run()
        .context(error::CommandStartSnafu)?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("{}", &stdout);
    ensure!(
        output.status.success(),
        error::DockerExecutionSnafu {
            tool: config.docker_tool,
            args: arg_string
        }
    );
    Ok(())
}

// =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^= =^..^=

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_extract_file_name_simple_url() {
        let result = extract_file_name("https://example.com/archive.tar.gz").unwrap();
        assert_eq!(result, PathBuf::from("archive.tar.gz"));
    }

    #[test]
    fn test_extract_file_name_nested_path() {
        let result =
            extract_file_name("https://github.com/org/repo/releases/download/v1.0/file.tar.xz")
                .unwrap();
        assert_eq!(result, PathBuf::from("file.tar.xz"));
    }

    #[test]
    fn test_extract_file_name_invalid_url() {
        assert!(extract_file_name("not-a-url").is_err());
    }

    #[test]
    fn test_go_script_template_substitution() {
        let script = GO_SCRIPT_TMPL
            .replace("__LOCAL_FILE_NAME__", "source-1.0.tar.gz")
            .replace("__MOD_DIR__", "")
            .replace("__OUTPUT__", "bundled-source-1.0.tar.gz");
        assert!(script.contains("tar tf \"source-1.0.tar.gz\""));
        assert!(script.contains("tar xf \"source-1.0.tar.gz\""));
        assert!(script.contains("tar czf \"bundled-source-1.0.tar.gz\""));
        assert!(script.contains("go mod vendor"));
    }

    #[test]
    fn test_rust_script_template_substitution() {
        let script = RUST_SCRIPT_TMPL
            .replace("__LOCAL_FILE_NAME__", "crate-2.0.tar.gz")
            .replace("__MOD_DIR__", "")
            .replace("__OUTPUT__", "bundled-crate-2.0.tar.gz");
        assert!(script.contains("tar tf \"crate-2.0.tar.gz\""));
        assert!(script.contains("tar xf \"crate-2.0.tar.gz\""));
        assert!(script.contains("tar czf \"bundled-crate-2.0.tar.gz\""));
        assert!(script.contains("cargo vendor --locked"));
        assert!(script.contains(".cargo/config.toml"));
    }

    #[test]
    fn test_script_template_with_mod_dir() {
        let script = GO_SCRIPT_TMPL
            .replace("__LOCAL_FILE_NAME__", "source.tar.gz")
            .replace("__MOD_DIR__", "subdir/module")
            .replace("__OUTPUT__", "bundled-source.tar.gz");
        assert!(script.contains("if [ -z \"subdir/module\" ]"));
    }

    #[test]
    fn test_go_config_values() {
        assert_eq!(GO_CONFIG.docker_tool, "docker-go");
        assert_eq!(GO_CONFIG.cache_dir, ".gomodcache");
        assert_eq!(GO_CONFIG.cache_arg_name, "--go-mod-cache");
        assert_eq!(GO_CONFIG.script_name, "docker-go-script.sh");
    }

    #[test]
    fn test_rust_config_values() {
        assert_eq!(RUST_CONFIG.docker_tool, "docker-cargo");
        assert_eq!(RUST_CONFIG.cache_dir, ".cargo");
        assert_eq!(RUST_CONFIG.cache_arg_name, "--cargo-home");
        assert_eq!(RUST_CONFIG.script_name, "docker-cargo-script.sh");
    }
}
