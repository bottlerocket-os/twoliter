#![cfg(test)]

use std::ffi::OsStr;
use std::path::PathBuf;
use tokio::process::Command;

mod twoliter_update;

pub const TWOLITER_PATH: &'static str = env!("CARGO_BIN_FILE_TWOLITER");

pub fn test_projects_dir() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.join("projects")
}

pub async fn run_command<I, S, E>(cmd: S, args: I, env: E) -> std::process::Output
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
        .await
        .expect("failed to execute process");

    println!("stdout: {}", String::from_utf8_lossy(&output.stdout));
    println!("stderr: {}", String::from_utf8_lossy(&output.stderr));
    output
}
