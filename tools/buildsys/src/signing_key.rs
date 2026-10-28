/*!
`buildsys with-signing-key -- <command>` runs a build with a kit module-signing key that exists
only while the build runs.

If no workspace package sets `module-signing = true`, the command runs unchanged and no key is
created. Otherwise:

- The SDK's `openssl` generates one RSA-4096 key and self-signed certificate in a container with no
  network, a read-only filesystem and no logs. They leave the container only through its stdout,
  so the Docker daemon needs no access to this host's filesystem.
- The wrapper writes them to a new 0700 directory on tmpfs, never on disk, and locks that
  directory for as long as it runs. A directory nobody holds the lock on was left by a wrapper
  that was killed outright, and the next run removes it.
- The command runs in the wrapper's process group with `BUILDSYS_MODULE_SIGNING_KEY` set to the key,
  so a signal sent to the whole group, such as Ctrl-C or a CI job's kill, reaches it directly. A
  signal sent to the wrapper alone is forwarded to it, and if the wrapper is killed outright, the
  kernel sends the command SIGTERM.
- The wrapper removes the key directory when the command exits, or on the first SIGINT, SIGTERM,
  SIGHUP or SIGQUIT, after which it dies of that signal so that a calling shell stops too. Any of
  these signals that was ignored when the wrapper started stays ignored, as under `nohup`.
- A stop signal sent to the wrapper alone, such as `kill -TSTP` on its PID, stops only the wrapper;
  the command keeps running, and the key stays until the wrapper continues and exits. Ctrl-Z stops
  the whole process group, so it does not have this problem.

The key is new for every run, so every build reruns the opted-in packages and every package that
depends on them, even when nothing else changed.
*/

pub(crate) mod error;

use crate::args::WithSigningKeyArgs;
use buildsys::manifest::module_signing_packages;
use error::Result;
use nix::fcntl::{Flock, FlockArg};
use nix::sys::signal::{kill, Signal};
use nix::unistd::{getgid, getuid, Pid};
use rand::Rng;
use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::iterator::Signals;
use snafu::{ensure, OptionExt, ResultExt};
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{self, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;

/// A new path for every key, so opted-in packages rebuild with each key.
const KEY_VAR: &str = "BUILDSYS_MODULE_SIGNING_KEY";

/// The BuildKit secret id, and the file name in the key directory.
const KEY_FILE: &str = "module-signing-key.pem";
/// Key directories are `key.<pid>.<random>` under the parent from `key_parent`.
const KEY_DIR_PREFIX: &str = "key.";

/// The kernel's `certs/default_x509.genkey` with this key's subject.
const X509_GENKEY: &str = "\
[ req ]
default_bits = 4096
distinguished_name = req_distinguished_name
prompt = no
string_mask = utf8only
x509_extensions = myexts

[ req_distinguished_name ]
CN = Bottlerocket module signing key

[ myexts ]
basicConstraints=critical,CA:FALSE
keyUsage=digitalSignature
subjectKeyIdentifier=hash
authorityKeyIdentifier=keyid
";

/// What the kernel's `certs/Makefile` uses for its own generated key.
const DIGEST: &str = "-sha512";
const VALIDITY_DAYS: &str = "36500";

const SIGNALS: [i32; 4] = [SIGINT, SIGTERM, SIGHUP, SIGQUIT];

/// PID of the running child, or 0.
static CHILD_PID: AtomicI32 = AtomicI32::new(0);
/// The first signal the wrapper received, or 0.
static RECEIVED: AtomicI32 = AtomicI32::new(0);

/// Run `args.command` with a kit module-signing key and return its exit code. If a signal
/// arrives, this removes the key and then dies of the signal instead of returning.
pub(crate) fn with_signing_key(args: WithSigningKeyArgs) -> Result<i32> {
    let packages =
        module_signing_packages(&args.cargo_metadata_path).context(error::PackagesSnafu)?;
    let (program, program_args) = args.command.split_first().context(error::NoCommandSnafu)?;

    if packages.is_empty() {
        let source = Command::new(program).args(program_args).exec();
        return Err(source).context(error::RunSnafu {
            program: program.to_string_lossy(),
        });
    }

    if std::env::var_os(KEY_VAR).is_some() {
        eprintln!(
            "buildsys: ignoring {KEY_VAR} from the environment; every build creates its own key"
        );
    }

    // Catch signals before the key directory exists, so that none can leave it behind.
    let signals = Signals::new(handled_signals()).context(error::SignalsSnafu)?;
    let key_dir = KeyDir::create(&key_parent()?)?;
    let signal_key_dir = key_dir.path.clone();
    thread::spawn(move || handle_signals(signals, signal_key_dir));

    let result = generate_and_run(
        &key_dir.path,
        &args.sdk_image,
        &packages,
        program,
        program_args,
    );
    remove_key_dir(&key_dir.path);
    eprintln!(
        "buildsys: removed module-signing key directory {}",
        key_dir.path.display()
    );
    drop(key_dir);

    let received = RECEIVED.load(Ordering::SeqCst);
    if received != 0 {
        // Die of the signal, as the command would have without this wrapper, so that a calling
        // shell stops too. This returns only if the signal's default action does not terminate.
        let _ = signal_hook::low_level::emulate_default_handler(received);
        return Ok(128 + received);
    }
    result.map(exit_code)
}

/// The signals in `SIGNALS` that the wrapper handles: all but any it inherited as ignored. An
/// ignored signal must stay ignored for the command too, as under `nohup`, and a handler would
/// reset it to the default action in the command.
fn handled_signals() -> Vec<i32> {
    SIGNALS.into_iter().filter(|&s| !is_ignored(s)).collect()
}

/// Whether `signal` is currently ignored.
fn is_ignored(signal: i32) -> bool {
    // SAFETY: with a null new action, sigaction only reads the current one into `current`.
    unsafe {
        let mut current: libc::sigaction = std::mem::zeroed();
        libc::sigaction(signal, std::ptr::null(), &mut current) == 0
            && current.sa_sigaction == libc::SIG_IGN
    }
}

/// Handle every signal in `signals` for as long as the wrapper runs: record and act on the first
/// by removing `key_dir`, and forward each to the running child.
fn handle_signals(mut signals: Signals, key_dir: PathBuf) {
    for signal in signals.forever() {
        if RECEIVED
            .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            remove_key_dir(&key_dir);
        }
        forward(signal);
    }
}

/// Generate the key in `dir`, then run `program` with `KEY_VAR` set to it.
fn generate_and_run(
    dir: &Path,
    sdk_image: &str,
    packages: &[String],
    program: &OsStr,
    program_args: &[OsString],
) -> Result<ExitStatus> {
    let pem = generate_key(sdk_image)?;
    let key = dir.join(KEY_FILE);
    write_key(&key, &pem)?;
    drop(pem);
    eprintln!(
        "buildsys: module-signing key for {} created in {}; it is removed when this build ends",
        packages.join(" "),
        dir.display()
    );

    let mut command = Command::new(program);
    command.args(program_args).env(KEY_VAR, &key);
    run_child(&mut command, |mut child| child.wait()).context(error::RunSnafu {
        program: program.to_string_lossy(),
    })
}

/// Generate the key and its self-signed certificate with the SDK's `openssl`, and return both as
/// one PEM file. The container has no network, no writable layer and no logs, and openssl reads
/// its config from stdin and writes the PEM to stdout, so nothing in the container touches a file
/// on this host. That also lets the daemon be one this host shares no filesystem with, such as the
/// host's daemon when Twoliter runs in a container.
fn generate_key(sdk_image: &str) -> Result<Vec<u8>> {
    let mut keygen = Command::new("docker");
    keygen
        .args(["run", "--rm", "--interactive", "--pull", "never"])
        .args(["--network", "none", "--log-driver", "none", "--read-only"])
        .arg("--user")
        .arg(format!("{}:{}", getuid(), getgid()))
        .arg(sdk_image)
        .args([
            "openssl", "req", "-new", "-x509", "-nodes", "-utf8", "-batch",
        ])
        .args([DIGEST, "-days", VALIDITY_DAYS, "-config", "/dev/stdin"])
        .args([
            "-outform",
            "PEM",
            "-out",
            "/dev/stdout",
            "-keyout",
            "/dev/stdout",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = run_child(&mut keygen, |mut child| {
        // The config is far smaller than a pipe buffer, so writing all of it before reading any
        // output cannot deadlock. Dropping stdin closes it, which ends openssl's config.
        let mut stdin = child.stdin.take().expect("stdin is piped");
        stdin.write_all(X509_GENKEY.as_bytes())?;
        drop(stdin);
        child.wait_with_output()
    })
    .context(error::KeyGenStartSnafu)?;
    ensure!(
        RECEIVED.load(Ordering::SeqCst) == 0,
        error::InterruptedSnafu
    );
    ensure!(
        output.status.success(),
        error::KeyGenSnafu {
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        }
    );
    check_key_pem(&output.stdout)?;
    Ok(output.stdout)
}

/// Check that `pem` holds both a private key and a certificate.
fn check_key_pem(pem: &[u8]) -> Result<()> {
    for (what, marker) in [
        ("private key", "-----BEGIN PRIVATE KEY-----"),
        ("certificate", "-----BEGIN CERTIFICATE-----"),
    ] {
        ensure!(
            pem.windows(marker.len()).any(|w| w == marker.as_bytes()),
            error::KeyGenOutputSnafu { what }
        );
    }
    Ok(())
}

/// Write `pem` to a new file at `path` that only its owner can read.
fn write_key(path: &Path, pem: &[u8]) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| file.write_all(pem))
        .context(error::WriteKeySnafu { path })
}

/// Spawn `command` in the wrapper's process group and wait for it with `wait`. While it runs, its
/// PID is in `CHILD_PID`, so that `forward` can pass on a signal sent to the wrapper alone. If a
/// signal has already arrived, the command is not started.
fn run_child<T>(
    command: &mut Command,
    wait: impl FnOnce(process::Child) -> io::Result<T>,
) -> io::Result<T> {
    if RECEIVED.load(Ordering::SeqCst) != 0 {
        return Err(io::Error::new(ErrorKind::Interrupted, "interrupted"));
    }
    die_with_wrapper(command);
    let child = command.spawn()?;
    CHILD_PID.store(child.id() as i32, Ordering::SeqCst);
    // A signal that arrived between the check above and the store was not forwarded.
    let received = RECEIVED.load(Ordering::SeqCst);
    if received != 0 {
        forward(received);
    }
    let result = wait(child);
    CHILD_PID.store(0, Ordering::SeqCst);
    result
}

/// Have the kernel send `command` SIGTERM if the wrapper dies first, as when it is killed outright,
/// so that the command cannot outlive the wrapper that would remove its key.
#[cfg(target_os = "linux")]
fn die_with_wrapper(command: &mut Command) {
    let wrapper = process::id();
    // SAFETY: the hook runs in the child between fork and exec, and calls only prctl and getppid,
    // which are async-signal-safe, and builds an `io::Error` without allocating.
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                return Err(io::Error::last_os_error());
            }
            // The wrapper died before the prctl took effect, so no signal will come.
            if libc::getppid() as u32 != wrapper {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
}

/// Off Linux there is no parent-death signal; the command can outlive a killed wrapper.
#[cfg(not(target_os = "linux"))]
fn die_with_wrapper(_command: &mut Command) {}

/// Send `signal` to the running child, if there is one. A signal sent to the whole process group
/// has reached the child already; this passes on one sent to the wrapper alone.
fn forward(signal: i32) {
    let pid = CHILD_PID.load(Ordering::SeqCst);
    if pid > 0 {
        if let Ok(signal) = Signal::try_from(signal) {
            let _ = kill(Pid::from_raw(pid), signal);
        }
    }
}

/// A 0700 directory on tmpfs, never on disk: under `$XDG_RUNTIME_DIR`, else `/dev/shm`.
fn key_parent() -> Result<PathBuf> {
    let uid = getuid().as_raw();
    let mut candidates = Vec::new();
    if let Some(runtime_dir) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) {
        if runtime_dir.is_absolute() && is_private_dir(&runtime_dir, uid) {
            candidates.push((
                runtime_dir.clone(),
                runtime_dir.join("twoliter-module-signing"),
            ));
        }
    }
    let shm = PathBuf::from("/dev/shm");
    candidates.push((
        shm.clone(),
        shm.join(format!("twoliter-module-signing-{uid}")),
    ));

    let (_, parent) = candidates
        .into_iter()
        .find(|(base, _)| is_tmpfs(base))
        .context(error::NoTmpfsSnafu)?;

    match DirBuilder::new().mode(0o700).create(&parent) {
        Ok(()) => fs::set_permissions(&parent, fs::Permissions::from_mode(0o700))
            .context(error::CreateDirSnafu { path: &parent })?,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
        Err(source) => return Err(source).context(error::CreateDirSnafu { path: &parent }),
    }
    ensure!(
        is_private_dir(&parent, uid),
        error::NotPrivateSnafu { path: parent, uid }
    );
    Ok(parent)
}

/// Whether `path` itself, not a symlink to it, is a directory owned by `uid` with mode 0700.
fn is_private_dir(path: &Path, uid: u32) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.is_dir() && m.uid() == uid && m.mode() & 0o7777 == 0o700)
        .unwrap_or(false)
}

/// Whether `path` is on a tmpfs, so that a key written there never reaches a disk.
#[cfg(target_os = "linux")]
fn is_tmpfs(path: &Path) -> bool {
    use nix::sys::statfs::{statfs, TMPFS_MAGIC};
    statfs(path)
        .map(|s| s.filesystem_type() == TMPFS_MAGIC)
        .unwrap_or(false)
}

/// No tmpfs check exists off Linux, so no path qualifies.
#[cfg(not(target_os = "linux"))]
fn is_tmpfs(_path: &Path) -> bool {
    false
}

/// This run's key directory, locked for as long as the value lives. The lock is on a descriptor
/// that the command does not inherit, so it ends when the wrapper does, however it ends.
struct KeyDir {
    path: PathBuf,
    _lock: Flock<File>,
}

impl KeyDir {
    /// Remove the stale key directories under `parent`, then create and lock a new one. Both
    /// happen under an exclusive lock on `parent`, so that a concurrent run cannot remove the new
    /// directory before it is locked.
    fn create(parent: &Path) -> Result<Self> {
        let _parent_lock = lock_dir(parent, FlockArg::LockExclusive)?;
        remove_stale_key_dirs(parent);
        let path = parent.join(format!(
            "{KEY_DIR_PREFIX}{}.{:016x}",
            process::id(),
            rand::rng().random::<u64>()
        ));
        DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .and_then(|()| fs::set_permissions(&path, fs::Permissions::from_mode(0o700)))
            .context(error::CreateDirSnafu { path: &path })?;
        let lock = lock_dir(&path, FlockArg::LockExclusiveNonblock)?;
        Ok(Self { path, _lock: lock })
    }
}

/// Open the directory `dir` and take `arg` on it.
fn lock_dir(dir: &Path, arg: FlockArg) -> Result<Flock<File>> {
    let file = File::open(dir).context(error::OpenDirSnafu { path: dir })?;
    Flock::lock(file, arg)
        .map_err(|(_, errno)| errno)
        .context(error::LockSnafu { path: dir })
}

/// Remove the key directories under `parent` that no running wrapper holds the lock on: each was
/// left by a wrapper that was killed before it could remove it.
fn remove_stale_key_dirs(parent: &Path) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_key_dir = entry
            .file_name()
            .to_string_lossy()
            .starts_with(KEY_DIR_PREFIX)
            && entry.file_type().is_ok_and(|t| t.is_dir());
        if is_key_dir && lock_dir(&path, FlockArg::LockExclusiveNonblock).is_ok() {
            eprintln!(
                "buildsys: removing stale module-signing key directory {}",
                path.display()
            );
            remove_key_dir(&path);
        }
    }
}

/// Remove a key directory, the key first. Both the signal thread and the main thread call this, so
/// files that are already gone are fine.
fn remove_key_dir(dir: &Path) {
    let ignore_missing = |result: io::Result<()>, path: &Path| match result {
        Err(e) if e.kind() != ErrorKind::NotFound => {
            eprintln!(
                "buildsys: warning: failed to remove {}: {e}",
                path.display()
            )
        }
        _ => {}
    };
    let key = dir.join(KEY_FILE);
    ignore_missing(fs::remove_file(&key), &key);
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            ignore_missing(fs::remove_file(entry.path()), &entry.path());
        }
    }
    ignore_missing(fs::remove_dir(dir), dir);
}

/// The child's exit code, or `128 + N` if signal N killed it.
fn exit_code(status: ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(1)
}

#[cfg(test)]
mod test {
    use super::*;
    use signal_hook::consts::SIGUSR1;
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    /// A new 0700 directory to hold key directories.
    fn private_parent() -> TempDir {
        let parent = TempDir::new().unwrap();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        parent
    }

    #[test]
    fn test_key_dir_create_returns_private_locked_dir() {
        let parent = private_parent();
        let key_dir = KeyDir::create(parent.path()).unwrap();
        assert!(is_private_dir(&key_dir.path, getuid().as_raw()));
        assert!(lock_dir(&key_dir.path, FlockArg::LockExclusiveNonblock).is_err());
    }

    #[test]
    fn test_key_dir_create_removes_unlocked_key_dirs() {
        let parent = private_parent();
        let stale = parent.path().join("key.1.0");
        fs::create_dir(&stale).unwrap();
        fs::write(stale.join(KEY_FILE), "secret").unwrap();
        let unrelated = parent.path().join("unrelated");
        fs::create_dir(&unrelated).unwrap();

        let _key_dir = KeyDir::create(parent.path()).unwrap();
        assert!(!stale.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn test_key_dir_create_keeps_locked_key_dirs() {
        let parent = private_parent();
        let first = KeyDir::create(parent.path()).unwrap();
        let second = KeyDir::create(parent.path()).unwrap();
        assert!(first.path.exists());
        assert_ne!(first.path, second.path);
    }

    #[test]
    fn test_key_dir_drop_unlocks_dir() {
        let parent = private_parent();
        let key_dir = KeyDir::create(parent.path()).unwrap();
        let path = key_dir.path.clone();
        drop(key_dir);
        assert!(lock_dir(&path, FlockArg::LockExclusiveNonblock).is_ok());
    }

    #[test]
    fn test_remove_key_dir_twice_succeeds() {
        let parent = private_parent();
        let key_dir = KeyDir::create(parent.path()).unwrap();
        fs::write(key_dir.path.join(KEY_FILE), "secret").unwrap();
        remove_key_dir(&key_dir.path);
        assert!(!key_dir.path.exists());
        remove_key_dir(&key_dir.path);
    }

    #[test]
    fn test_is_private_dir_0700_returns_true() {
        let parent = private_parent();
        assert!(is_private_dir(parent.path(), getuid().as_raw()));
    }

    #[test]
    fn test_is_private_dir_group_readable_returns_false() {
        let parent = private_parent();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o750)).unwrap();
        assert!(!is_private_dir(parent.path(), getuid().as_raw()));
    }

    #[test]
    fn test_is_private_dir_other_owner_returns_false() {
        let parent = private_parent();
        assert!(!is_private_dir(parent.path(), getuid().as_raw() + 1));
    }

    #[test]
    fn test_is_private_dir_symlink_returns_false() {
        let parent = private_parent();
        let target = parent.path().join("target");
        fs::create_dir(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
        let link = parent.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(!is_private_dir(&link, getuid().as_raw()));
    }

    #[test]
    fn test_handled_signals_ignored_signal_is_skipped() {
        // SAFETY: setting a disposition to SIG_IGN or SIG_DFL installs no handler code.
        unsafe { libc::signal(SIGHUP, libc::SIG_IGN) };
        let handled = handled_signals();
        unsafe { libc::signal(SIGHUP, libc::SIG_DFL) };
        assert!(!handled.contains(&SIGHUP));
        assert!(handled.contains(&SIGINT));
    }

    #[test]
    fn test_handle_signals_first_signal_removes_key_dir() {
        let parent = private_parent();
        let key_dir = KeyDir::create(parent.path()).unwrap();
        fs::write(key_dir.path.join(KEY_FILE), "secret").unwrap();
        // SIGUSR1 stands in for SIGTERM, which would end the test process if the handler were
        // not installed yet when it arrives.
        let signals = Signals::new([SIGUSR1]).unwrap();
        let path = key_dir.path.clone();
        thread::spawn(move || handle_signals(signals, path));

        signal_hook::low_level::raise(SIGUSR1).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while key_dir.path.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(!key_dir.path.exists());
        assert_eq!(RECEIVED.swap(0, Ordering::SeqCst), SIGUSR1);
    }

    #[test]
    fn test_check_key_pem_key_and_certificate_succeeds() {
        let pem = b"-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----\n\
            -----BEGIN CERTIFICATE-----\ny\n-----END CERTIFICATE-----\n";
        assert!(check_key_pem(pem).is_ok());
    }

    #[test]
    fn test_check_key_pem_without_certificate_returns_error() {
        let pem = b"-----BEGIN PRIVATE KEY-----\nx\n-----END PRIVATE KEY-----\n";
        assert!(matches!(
            check_key_pem(pem),
            Err(error::Error::KeyGenOutput {
                what: "certificate"
            })
        ));
    }

    #[test]
    fn test_exit_code_exited_returns_code() {
        assert_eq!(exit_code(ExitStatus::from_raw(3 << 8)), 3);
    }

    #[test]
    fn test_exit_code_signaled_returns_128_plus_n() {
        assert_eq!(exit_code(ExitStatus::from_raw(SIGTERM)), 128 + SIGTERM);
    }
}
