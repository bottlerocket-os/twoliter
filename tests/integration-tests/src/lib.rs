#![cfg(test)]

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

mod twoliter_update;

pub const TWOLITER_PATH: &'static str = env!("CARGO_BIN_FILE_TWOLITER");

pub fn test_projects_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.join("projects")
}

pub fn run_command<I, S, E>(cmd: S, args: I, env: E) -> std::process::Output
where
    I: IntoIterator<Item = S>,
    E: IntoIterator<Item = (S, S)>,
    S: AsRef<OsStr>,
{
    let args: Vec<S> = args.into_iter().collect();

    println!(
        "Executing '{}' with args [{}]",
        cmd.as_ref().to_string_lossy(),
        args.iter()
            .map(|arg| format!("'{}'", arg.as_ref().to_string_lossy()))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let output = Command::new(cmd)
        .args(args.into_iter())
        .envs(env.into_iter())
        .output()
        .expect("failed to execute process");

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    output
}

struct KitRegistry {
    temp_dir: TempDir,
    container_id: String,
}

impl KitRegistry {
    fn new() -> Self {
        let temp_dir = TempDir::new().expect("failed to create path for oci registry spinup");

        let cert_dir = temp_dir.path().join("certs");
        let cert_file = cert_dir.join("registry.crt");
        std::fs::create_dir_all(&cert_dir).expect("failed to create nginx dir");
        let output = run_command(
            "openssl",
            [
                "req",
                "-x509",
                "-nodes",
                "-days",
                "365",
                "-newkey",
                "rsa:2048",
                "-keyout",
                cert_dir.join("registry.key").to_str().unwrap(),
                "-out",
                cert_file.to_str().unwrap(),
                "-batch",
                "-addext",
                "subjectAltName=DNS:localhost",
            ],
            [],
        );
        assert!(
            output.status.success(),
            "generate openssl self-signed certificates"
        );

        let output = run_command(
            "docker",
            [
                "run",
                "-d",
                "--rm",
                "--volume",
                "./certs:/auth/certs",
                "-e REGISTRY_HTTP_RELATIVEURLS=true",
                "-e REGISTRY_HTTP_ADDR=0.0.0.0:5000",
                "-e REGISTRY_HTTP_TLS_CERTIFICATE=/auth/certs/registry.crt",
                "-e REGISTRY_HTTP_TLS_KEY=/auth/certs/registry.key",
                "-p",
                "5000:5000",
                "public.ecr.aws/docker/library/registry:2.8.3",
            ],
            [],
        );
        assert!(output.status.success(), "failed to start oci registry");
        let container_id = String::from_utf8(output.stdout).unwrap().trim().to_string();

        Self {
            temp_dir,
            container_id,
        }
    }

    fn cert_file(&self) -> PathBuf {
        self.temp_dir
            .path()
            .join("certs/registry.crt")
            .to_path_buf()
    }
}

impl Drop for KitRegistry {
    fn drop(&mut self) {
        let output = run_command("docker", ["kill", &self.container_id], []);
        assert!(output.status.success(), "failed to stop oci registry");
    }
}
