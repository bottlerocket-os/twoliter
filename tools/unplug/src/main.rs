//! Unplug is a command-line tool to run another program without network access.
//! It applies a seccomp filter that restricts most socket-related syscalls.

use std::env;
#[cfg(target_os = "linux")]
use std::process::Command;
use std::process::ExitCode;

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::Result;

#[cfg(target_os = "linux")]
use seccompiler::*;

#[cfg(target_os = "linux")]
fn create_network_filter() -> Result<SeccompFilter> {
    let arch = std::env::consts::ARCH;
    Ok(SeccompFilter::new(
        // Only allow Unix domain sockets to be created. This may prove too limiting over time, but
        // avoids the need to filter the other syscalls that can be used once a socket exists.
        vec![(
            libc::SYS_socket,
            vec![SeccompRule::new(vec![SeccompCondition::new(
                1,
                SeccompCmpArgLen::Dword,
                SeccompCmpOp::Ne,
                libc::AF_UNIX as u64,
            )?])?],
        )]
        .into_iter()
        .collect(),
        // Allow the action if it doesn't match the syscall filter. "Allow by default" is unusual
        // in security contexts, but the goal is just to block network traffic to force external
        // dependencies to be pinned correctly and retrieved through supported mechanisms.
        SeccompAction::Allow,
        // Deny the action with a "Network is down" error if it does. This is chosen for its
        // relative rarity: it should be easier to trace the cause back to this seccomp profile,
        // unlike more common errors like EPERM's "Permission denied".
        SeccompAction::Errno(libc::ENETDOWN as u32),
        // Create the filter for the current architecture.
        arch.try_into()
            .with_context(|| format!("unsupported CPU architecture {arch}"))?,
    )?)
}

#[cfg(target_os = "linux")]
fn run(args: env::Args) -> Result<ExitCode> {
    let network_filter = create_network_filter().context("failed to create network filter")?;
    let bpf_program: BpfProgram = network_filter
        .try_into()
        .context("failed to compile network filter")?;

    apply_filter(&bpf_program).context("failed to apply network filter")?;

    let mut args = args.skip(1);
    if let Some(program) = args.next() {
        let ret = Command::new(&program)
            .args(args)
            .status()
            .with_context(|| format!("failed to run {program}"))?;
        let code = ret.code().unwrap_or(1) as u8;
        return Ok(code.into());
    }

    Ok(ExitCode::SUCCESS)
}

#[cfg(not(target_os = "linux"))]
fn run(_: env::Args) -> Result<ExitCode> {
    unimplemented!("unplug is not supported on this operating system");
}

fn main() -> Result<ExitCode> {
    run(env::args())
}
