use crate::{
    api::{
        envelope_output_string,
        with_envelope_metadata,
    },
    error::ConfigError,
};
use alloy_primitives::Address;
use chrono::{
    DateTime,
    Utc,
};
use clap::Parser;
use colored::Colorize;
use dirs::home_dir;
use pcl_common::args::{
    CliArgs,
    OutputMode,
    current_output_mode,
};
use serde::{
    Deserialize,
    Serialize,
};
use serde_json::{
    Value,
    json,
};

use std::{
    collections::BTreeMap,
    fmt,
    fs::OpenOptions,
    io::{
        Read,
        Seek,
        SeekFrom,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
};
use tokio::time::{
    Duration,
    Instant,
    sleep,
};
use uuid::Uuid;

/// Legacy directory name for storing PCL configuration (deprecated)
const LEGACY_CONFIG_DIR: &str = ".pcl";
/// Directory name for storing PCL configuration under XDG config home
const CONFIG_DIR_NAME: &str = "pcl";
/// Configuration file name
pub const CONFIG_FILE: &str = "config.toml";
pub const AUTH_EXPIRES_SOON_SECONDS: i64 = 300;

/// How long to wait for another process to release the config lock.
const CONFIG_LOCK_TIMEOUT: Duration = Duration::from_secs(30);
/// How often to retry while the config lock is held.
const CONFIG_LOCK_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Cross-process guard over the config file, held for a whole
/// read-modify-write.
///
/// Compare-and-write ([`CliConfig::write_to_file_if_unchanged`]) is not enough
/// on its own: it narrows the window between reading and writing but cannot
/// close it, so two processes can still interleave and the loser's write is
/// simply dropped. That is tolerable for a passive rewrite, and *not* tolerable
/// for the refresh token — there is only ever one valid refresh token, so
/// losing the write that recorded it logs the user out.
///
/// So every writer that merges into whatever is currently on disk takes this
/// lock, and they all take the *same* one: token refresh and platform selection
/// are different operations on the same file, and a lock that only refresh
/// honoured would not serialize them against each other.
///
/// The lock file name is the one older CLIs create with `create_new` and
/// delete on drop. This holder does the same, and also keeps an OS advisory
/// lock on the descriptor until drop. A clean exit removes the file, so an
/// older CLI's `create_new` succeeds instead of waiting out its timeout. A
/// killed process leaves the file, but the kernel releases the advisory lock;
/// the next newer CLI removes that file only when its recorded pid is dead
/// and only while this process still holds the advisory lock on that inode.
/// An existing file whose pid is still running is an older writer that already
/// dropped its descriptor: we wait, and we do not enter.
///
/// The lock path is never opened through a symlink. Metadata is written only
/// to the regular file this process just created, after checking that the
/// path still names that inode. A probe that grabs the advisory lock before
/// the creator does is retried; the creator does not delete its own file.
#[derive(Debug)]
pub struct ConfigLock {
    path: PathBuf,
    /// Held so the descriptor, and the advisory lock on it, stay open.
    file: std::fs::File,
}

/// Creates `dir` and any missing parents. Directories created here are 0700
/// on unix (they hold credentials and pending login secrets); an existing
/// directory keeps its mode.
pub(crate) fn create_private_dir_all(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

impl ConfigLock {
    /// Acquires the lock, waiting up to 30 seconds for another process to
    /// release it before returning [`ConfigError::LockTimeout`].
    pub async fn acquire(cli_args: &CliArgs) -> Result<Self, ConfigError> {
        let path = CliConfig::config_file_path(cli_args).with_extension("toml.lock");
        if let Some(parent) = path.parent() {
            create_private_dir_all(parent).map_err(ConfigError::WriteError)?;
        }
        let deadline = Instant::now() + CONFIG_LOCK_TIMEOUT;
        loop {
            if let Some(lock) = try_create_lock(&path, deadline).await? {
                return Ok(lock);
            }
            match classify_existing_lock(&path)? {
                ExistingLock::Gone => {}
                ExistingLock::Busy => {
                    if Instant::now() >= deadline {
                        return Err(ConfigError::LockTimeout);
                    }
                    sleep(CONFIG_LOCK_POLL_INTERVAL).await;
                }
            }
        }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        // Unlink only the inode we created. A replacement at the same path
        // belongs to whoever holds it now.
        if opened_is_this_regular_file(&self.file, &self.path).unwrap_or(false) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// What an existing `config.toml.lock` means to a newer CLI.
enum ExistingLock {
    /// The path is gone, or it was a symlink or a dead holder's file and that
    /// name was unlinked.
    Gone,
    /// A live holder: either a newer CLI's advisory lock, or an older CLI
    /// whose pid is still running after it closed the descriptor.
    Busy,
}

/// Creates the lock the way an older CLI does (`create_new`), then keeps the
/// descriptor and its advisory lock. `Ok(None)` means the path already exists.
///
/// `WouldBlock` from the advisory lock is a contender's probe. The file this
/// function just created stays in place and the lock is retried until
/// `deadline`.
async fn try_create_lock(
    path: &Path,
    deadline: Instant,
) -> Result<Option<ConfigLock>, ConfigError> {
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
        Err(error) => return Err(ConfigError::WriteError(error)),
    };
    if !opened_is_this_regular_file(&file, path).unwrap_or(false) {
        return Err(ConfigError::WriteError(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config lock path is not the file just created",
        )));
    }
    let _ = writeln!(
        file,
        "pid={} acquired_at={}",
        std::process::id(),
        Utc::now().to_rfc3339()
    );
    let _ = file.sync_all();
    #[cfg(test)]
    pause_until_lock_release("PCL_LOCK_BEFORE_FLOCK", "PCL_LOCK_BEFORE_FLOCK_RELEASE");
    loop {
        match try_lock_exclusive(&file) {
            Ok(()) => {
                if !opened_is_this_regular_file(&file, path).unwrap_or(false) {
                    return Err(ConfigError::WriteError(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "config lock path changed before the advisory lock was held",
                    )));
                }
                return Ok(Some(ConfigLock {
                    path: path.to_path_buf(),
                    file,
                }));
            }
            Err(error) if lock_is_held_elsewhere(&error) => {
                if Instant::now() >= deadline {
                    // The probe outlived the wait. Remove only the inode we
                    // created, so a replacement at this path is left alone.
                    if opened_is_this_regular_file(&file, path).unwrap_or(false) {
                        let _ = std::fs::remove_file(path);
                    }
                    return Err(ConfigError::LockTimeout);
                }
                sleep(CONFIG_LOCK_POLL_INTERVAL).await;
            }
            Err(error) => {
                if opened_is_this_regular_file(&file, path).unwrap_or(false) {
                    let _ = std::fs::remove_file(path);
                }
                return Err(ConfigError::WriteError(error));
            }
        }
    }
}

fn classify_existing_lock(path: &Path) -> Result<ExistingLock, ConfigError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ExistingLock::Gone);
        }
        Err(error) => return Err(ConfigError::WriteError(error)),
    };
    if meta.file_type().is_symlink() {
        return unlink_symlink_lock(path);
    }
    if !meta.is_file() {
        return Err(ConfigError::WriteError(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config lock path is not a regular file",
        )));
    }
    let file = match open_nofollow_read(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ExistingLock::Gone);
        }
        Err(error) if is_nofollow_rejection(&error) => return unlink_symlink_lock(path),
        Err(error) => return Err(ConfigError::WriteError(error)),
    };
    // The holder can unlink or replace the file between this open and the
    // identity check. A vanished path is retried immediately. A regular-file
    // replacement is left alone and retried within the acquire deadline.
    // Permission and other I/O failures, and a path that is not a regular
    // file, stay errors.
    #[cfg(test)]
    pause_until_lock_release("PCL_LOCK_AFTER_OPEN", "PCL_LOCK_AFTER_OPEN_RELEASE");
    match classify_opened_lock(path, &file)? {
        OpenedLock::Same => {}
        OpenedLock::Vanished => return Ok(ExistingLock::Gone),
        OpenedLock::Replaced => return Ok(ExistingLock::Busy),
        OpenedLock::Symlink => return unlink_symlink_lock(path),
    }
    // A live pid, or a file that does not name a pid yet, is a holder. Do not
    // take the exclusive lock: that probe makes the creator's own `flock`
    // fail and used to unlink the file it had just created.
    if !holder_pid_is_dead(&file) {
        return Ok(ExistingLock::Busy);
    }
    // `file` stays open through the unlink below. Closing it first would
    // release the advisory lock and let a replacement appear under `remove_file`.
    let reclaimed = reclaim_dead_lock(path, &file);
    drop(file);
    reclaimed
}

/// How an already-opened lock relates to the path after the holder may have
/// exited or been replaced.
enum OpenedLock {
    /// `path` still names the opened inode.
    Same,
    /// The holder unlinked the path. The next attempt may create it.
    Vanished,
    /// `path` names a different regular file. That file is not ours.
    Replaced,
    /// `path` is now a symlink. Unlink the link, not its target.
    Symlink,
}

fn classify_opened_lock(path: &Path, file: &std::fs::File) -> Result<OpenedLock, ConfigError> {
    let opened = file.metadata().map_err(ConfigError::WriteError)?;
    if !opened.is_file() {
        return Err(ConfigError::WriteError(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "opened config lock is not a regular file",
        )));
    }
    let on_path = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(OpenedLock::Vanished);
        }
        Err(error) => return Err(ConfigError::WriteError(error)),
    };
    if on_path.file_type().is_symlink() {
        return Ok(OpenedLock::Symlink);
    }
    if !on_path.is_file() {
        return Err(ConfigError::WriteError(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "config lock path is not a regular file",
        )));
    }
    if same_file(&opened, &on_path) {
        Ok(OpenedLock::Same)
    } else {
        Ok(OpenedLock::Replaced)
    }
}

/// Unlinks a dead holder's lock while `file`'s advisory lock is held.
///
/// The lock is acquired before the path is checked again, and the caller
/// keeps `file` open until this returns. Dropping it first lets another
/// reaper create a replacement that `remove_file` would then delete.
fn reclaim_dead_lock(path: &Path, file: &std::fs::File) -> Result<ExistingLock, ConfigError> {
    match try_lock_exclusive(file) {
        Ok(()) => {}
        Err(error) if lock_is_held_elsewhere(&error) => return Ok(ExistingLock::Busy),
        Err(error) => return Err(ConfigError::WriteError(error)),
    }
    if !stale_lock_still_ours(file, path) {
        return Ok(ExistingLock::Busy);
    }
    #[cfg(test)]
    pause_until_lock_release(
        "PCL_LOCK_BEFORE_REAP_UNLINK",
        "PCL_LOCK_BEFORE_REAP_UNLINK_RELEASE",
    );
    if !stale_lock_still_ours(file, path) {
        return Ok(ExistingLock::Busy);
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ConfigError::WriteError(error)),
    }
    Ok(ExistingLock::Gone)
}

fn stale_lock_still_ours(file: &std::fs::File, path: &Path) -> bool {
    opened_is_this_regular_file(file, path).unwrap_or(false) && holder_pid_is_dead(file)
}

/// Test hook. The creator or reaper writes `ready_var` and waits until
/// `release_var` exists, so a contender can run at that exact point.
#[cfg(test)]
fn pause_until_lock_release(ready_var: &str, release_var: &str) {
    let Ok(ready) = std::env::var(ready_var) else {
        return;
    };
    let Ok(release) = std::env::var(release_var) else {
        return;
    };
    if std::fs::write(&ready, "paused").is_err() {
        return;
    }
    let release = PathBuf::from(release);
    while !release.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Removes a symlink at the lock path. `remove_file` unlinks the symlink
/// itself and does not follow it, so the target is left untouched.
fn unlink_symlink_lock(path: &Path) -> Result<ExistingLock, ConfigError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::remove_file(path).map_err(ConfigError::WriteError)?;
            Ok(ExistingLock::Gone)
        }
        Ok(_) => Ok(ExistingLock::Busy),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ExistingLock::Gone),
        Err(error) => Err(ConfigError::WriteError(error)),
    }
}

fn holder_pid_is_dead(file: &std::fs::File) -> bool {
    lock_file_holder_pid(file).is_some_and(|pid| !process_is_alive(pid))
}

fn lock_file_holder_pid(file: &std::fs::File) -> Option<u32> {
    let mut file = file.try_clone().ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    let rest = text.trim().strip_prefix("pid=")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// True when `file` is a regular file and `path` still names that same inode.
/// `symlink_metadata` does not follow a symlink, so a path that points
/// somewhere else does not compare equal.
fn opened_is_this_regular_file(file: &std::fs::File, path: &Path) -> std::io::Result<bool> {
    let opened = file.metadata()?;
    if !opened.is_file() {
        return Ok(false);
    }
    let on_path = std::fs::symlink_metadata(path)?;
    if !on_path.file_type().is_file() {
        return Ok(false);
    }
    Ok(same_file(&opened, &on_path))
}

#[cfg(unix)]
fn same_file(opened: &std::fs::Metadata, on_path: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    opened.dev() == on_path.dev() && opened.ino() == on_path.ino()
}

#[cfg(windows)]
fn same_file(opened: &std::fs::Metadata, on_path: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    opened.volume_serial_number() == on_path.volume_serial_number()
        && opened.file_index() == on_path.file_index()
}

#[cfg(not(any(unix, windows)))]
fn same_file(_opened: &std::fs::Metadata, _on_path: &std::fs::Metadata) -> bool {
    true
}

fn open_nofollow_read(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "tvos",
        target_os = "watchos",
        target_os = "freebsd",
        target_os = "dragonfly",
    ))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Open the reparse point itself instead of its target.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(path)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
const O_NOFOLLOW: i32 = 0x20000;
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "tvos",
    target_os = "watchos"
))]
const O_NOFOLLOW: i32 = 0x0100;
#[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
const O_NOFOLLOW: i32 = 0x0000_0200;

fn is_nofollow_rejection(error: &std::io::Error) -> bool {
    // `ErrorKind::FilesystemLoop` is still unstable. ELOOP is 40 on
    // Linux/Android and 62 on macOS, iOS, and the BSDs.
    matches!(error.raw_os_error(), Some(40 | 62))
}

/// `pid` still has a process. A pid we cannot prove dead is treated as alive,
/// so a leftover file is not stolen out from under an older writer.
#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    // SAFETY: signal 0 does not deliver a signal. It only checks that `pid`
    // names a process this caller may signal.
    let rc = unsafe { kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    // ESRCH (3) is "no such process". EPERM means the process exists.
    !matches!(std::io::Error::last_os_error().raw_os_error(), Some(3))
}

#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut core::ffi::c_void;
        fn CloseHandle(handle: *mut core::ffi::c_void) -> i32;
        fn GetLastError() -> u32;
    }
    if pid == 0 {
        return true;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const ERROR_INVALID_PARAMETER: u32 = 87;
    // SAFETY: a null handle is the failure case and is not closed. A non-null
    // handle is closed before return.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if !handle.is_null() {
        // SAFETY: `handle` came from OpenProcess and is closed once.
        unsafe { CloseHandle(handle) };
        return true;
    }
    // SAFETY: GetLastError reads the calling thread's last error and takes no pointer.
    unsafe { GetLastError() != ERROR_INVALID_PARAMETER }
}

#[cfg(not(any(unix, windows)))]
fn process_is_alive(_pid: u32) -> bool {
    true
}

/// `flock` / `LockFileEx` report a live holder with these errors when asked
/// not to wait. Both locks die with the holding process.
fn lock_is_held_elsewhere(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || matches!(error.raw_os_error(), Some(11 | 35 | 33))
}

#[cfg(unix)]
fn try_lock_exclusive(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // BSD `flock` operation bits. Linux and macOS share these values.
    const LOCK_EX: i32 = 2;
    const LOCK_NB: i32 = 4;
    unsafe extern "C" {
        fn flock(fd: std::os::unix::io::RawFd, operation: i32) -> i32;
    }
    // SAFETY: `fd` belongs to `file` and outlives this call. `LOCK_EX|LOCK_NB`
    // does not close it. The kernel drops the lock when the descriptor closes,
    // including when the process is killed.
    let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn try_lock_exclusive(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut core::ffi::c_void,
    }
    unsafe extern "system" {
        fn LockFileEx(
            file: *mut core::ffi::c_void,
            flags: u32,
            reserved: u32,
            bytes_low: u32,
            bytes_high: u32,
            overlapped: *mut Overlapped,
        ) -> i32;
    }
    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 1;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 2;
    let mut overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: std::ptr::null_mut(),
    };
    // SAFETY: the handle belongs to `file`. A zero byte count locks from
    // offset 0 through the end of the file. The overlapped record is live for
    // the call. The kernel drops the lock when the handle closes.
    let locked = unsafe {
        LockFileEx(
            file.as_raw_handle(),
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            0,
            0,
            &mut overlapped,
        )
    };
    if locked == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
fn try_lock_exclusive(_file: &std::fs::File) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the config lock requires unix or windows",
    ))
}

/// Main configuration structure for PCL
///
/// This struct holds all the configuration data for the PCL tool,
/// including authentication details.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CliConfig {
    /// Optional authentication details
    pub auth: Option<UserAuth>,
    /// Platform URL remembered from the last `pcl auth login` or interactive
    /// network selection.
    ///
    /// Always set explicitly once a platform has been chosen — there is no
    /// production default to encode as an absent value — and cleared on
    /// `pcl auth logout`. Resolution reads this after an explicit
    /// `-u`/`PCL_API_URL` and before prompting, so a platform only has to be
    /// chosen once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform_url: Option<String>,
    /// Per-chain RPC endpoints used when broadcasting transactions.
    /// Keys are chain ids as strings (TOML table keys must be strings).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rpc: BTreeMap<String, RpcEndpoint>,
}

/// RPC endpoint configuration for a single chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcEndpoint {
    /// HTTP(S) RPC URL for the chain
    pub url: String,
    /// Confirmations to wait for after broadcasting (defaults to 1 when absent)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmations: Option<u64>,
}

impl CliConfig {
    /// Returns the configured RPC endpoint for a chain id, if any.
    pub fn rpc_endpoint(&self, chain_id: u64) -> Option<&RpcEndpoint> {
        self.rpc.get(&chain_id.to_string())
    }

    /// Records `platform_url` as both the remembered platform and the issuer of
    /// the stored credentials — the state a completed login leaves behind.
    ///
    /// Test-only, and deliberately one call: a fixture that sets only the
    /// remembered platform describes credentials of unknown provenance, which
    /// the platform-boundary check refuses. Tests that mean *that* state set
    /// `platform_url` on its own.
    #[cfg(test)]
    pub(crate) fn set_test_platform(&mut self, platform_url: &str) {
        self.platform_url = Some(platform_url.to_string());
        if let Some(auth) = &mut self.auth {
            auth.issuer_platform_url = Some(platform_url.to_string());
        }
    }
}

/// Command-line arguments for configuration management
#[derive(Parser)]
pub struct ConfigArgs {
    #[command(subcommand)]
    command: ConfigCommand,
}

/// Subcommands for configuration management
#[derive(clap::Subcommand)]
enum ConfigCommand {
    #[command(about = "Display the current configuration")]
    Show,
    #[command(about = "Delete the current configuration")]
    Delete,
    #[command(
        name = "set-rpc",
        about = "Set the RPC endpoint used when broadcasting transactions on a chain"
    )]
    SetRpc {
        /// Chain id the endpoint serves
        chain_id: u64,
        /// HTTP(S) RPC URL
        url: String,
        /// Confirmations to wait for after broadcasting (default 1)
        #[arg(long)]
        confirmations: Option<u64>,
    },
    #[command(
        name = "unset-rpc",
        about = "Remove the stored RPC endpoint for a chain"
    )]
    UnsetRpc {
        /// Chain id to remove
        chain_id: u64,
    },
}

impl ConfigArgs {
    pub fn can_run_without_valid_config(&self) -> bool {
        matches!(self.command, ConfigCommand::Delete)
    }

    pub fn should_force_config_write(&self) -> bool {
        matches!(self.command, ConfigCommand::Delete)
    }

    /// Executes the configuration command
    ///
    /// # Arguments
    /// * `config` - The configuration to operate on
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    pub fn run(&self, config: &mut CliConfig, cli_args: &CliArgs) -> Result<(), ConfigError> {
        match &self.command {
            ConfigCommand::SetRpc {
                chain_id,
                url,
                confirmations,
            } => {
                let parsed = url::Url::parse(url).map_err(|e| {
                    ConfigError::InvalidValue(format!(
                        "invalid RPC URL {}: {e}",
                        redacted_rpc_host(url)
                    ))
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(ConfigError::InvalidValue(format!(
                        "RPC URL must be http(s), got {:?}",
                        parsed.scheme()
                    )));
                }
                config.rpc.insert(
                    chain_id.to_string(),
                    RpcEndpoint {
                        url: url.clone(),
                        confirmations: *confirmations,
                    },
                );
                // Echo only non-secret metadata; the URL may embed an API key.
                print_config_output(
                    &with_envelope_metadata(json!({
                        "status": "ok",
                        "data": {
                            "chain_id": chain_id,
                            "rpc": {
                                "configured": true,
                                "host": redacted_rpc_host(url),
                                "confirmations": confirmations,
                            },
                        },
                        "next_actions": ["pcl config show"],
                    })),
                    cli_args.json_output(),
                )
            }
            ConfigCommand::UnsetRpc { chain_id } => {
                let removed = config.rpc.remove(&chain_id.to_string());
                print_config_output(
                    &with_envelope_metadata(json!({
                        "status": "ok",
                        "data": {
                            "chain_id": chain_id,
                            "removed": removed.is_some(),
                        },
                        "next_actions": ["pcl config show"],
                    })),
                    cli_args.json_output(),
                )
            }
            ConfigCommand::Show => {
                print_config_output(
                    &config_show_envelope(config, cli_args),
                    cli_args.json_output(),
                )
            }
            ConfigCommand::Delete => {
                *config = CliConfig::default();
                print_config_output(
                    &json!({
                        "status": "ok",
                        "data": {
                            "deleted": true,
                            "config_path": CliConfig::config_file_path(cli_args).display().to_string(),
                            "auth": config_auth_value(config),
                        },
                        "next_actions": [
                            "pcl auth login",
                            "pcl config show",
                        ],
                    }),
                    cli_args.json_output(),
                )
            }
        }
    }
}

impl CliConfig {
    /// Updates stored auth expiry from the JWT `exp` claim when available.
    ///
    /// Older CLI versions stored the short device-login session expiry here,
    /// which made valid tokens look expired after only a few minutes.
    pub fn normalize_auth_expiry_from_access_token(&mut self) -> bool {
        let Some(auth) = &mut self.auth else {
            return false;
        };
        let Some(token_expires_at) = auth.access_token_expires_at() else {
            return false;
        };
        if auth.expires_at == token_expires_at {
            return false;
        }
        auth.expires_at = token_expires_at;
        true
    }

    /// Returns the path to the active config file for the supplied CLI arguments.
    pub fn config_file_path(cli_args: &CliArgs) -> PathBuf {
        cli_args
            .config_dir
            .clone()
            .unwrap_or(Self::get_config_dir())
            .join(CONFIG_FILE)
    }

    /// Writes the configuration to the default config file, or a specific directory
    ///
    /// # Arguments
    /// * `cli_args` - Command line arguments
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    pub fn write_to_file(&self, cli_args: &CliArgs) -> Result<(), ConfigError> {
        self.write_to_file_at_dir(
            &cli_args
                .config_dir
                .clone()
                .unwrap_or(Self::get_config_dir()),
        )
    }

    /// Records this process's chosen platform without clobbering a concurrent
    /// write, then adopts whatever ended up on disk.
    ///
    /// The in-memory config was read before the network selector prompted, and
    /// the prompt stays open for as long as the user takes to answer. Another
    /// `pcl` can rotate the refresh token in that window, so this process's
    /// snapshot must not be written wholesale — only `platform_url` is merged
    /// into the current file.
    ///
    /// The read and the write are both inside a [`ConfigLock`] because merging
    /// is only safe if nothing else writes in between: a token refresh landing
    /// after the read would be overwritten by the write, discarding the only
    /// valid refresh token. Holding the same lock refresh takes makes the two
    /// operations mutually exclusive rather than merely narrow.
    pub async fn merge_selected_platform(&mut self, cli_args: &CliArgs) -> Result<(), ConfigError> {
        let _lock = ConfigLock::acquire(cli_args).await?;
        let Ok(mut current) = Self::read_from_file(cli_args) else {
            // Unreadable only if it changed under us since startup, where the
            // caller already parsed it. Nothing to merge, so record the choice.
            return self.write_to_file(cli_args);
        };
        if current == *self {
            return Ok(());
        }
        current.normalize_auth_expiry_from_access_token();
        current.platform_url.clone_from(&self.platform_url);
        current.write_to_file(cli_args)?;
        *self = current;
        Ok(())
    }

    /// Writes the configuration only when the on-disk config still matches
    /// the snapshot read at process start. This prevents a read-only command
    /// from overwriting credentials that another process just refreshed.
    pub fn write_to_file_if_unchanged(
        &self,
        cli_args: &CliArgs,
        expected_current: &Self,
    ) -> Result<bool, ConfigError> {
        let current = Self::read_from_file(cli_args)?;
        if current != *expected_current {
            return Ok(false);
        }
        self.write_to_file(cli_args)?;
        Ok(true)
    }

    /// Copies an unreadable config file aside before it is replaced.
    ///
    /// A repair command (`pcl auth login`, `pcl config delete`) is allowed to
    /// write a fresh config over a file it could not parse. Those bytes may
    /// still hold recoverable credentials or RPC settings, so they are kept at
    /// `config.toml.invalid` rather than dropped. Returns the backup path, or
    /// `None` when there was no file to preserve.
    ///
    /// Only ever reached while the current file is unparseable, so it cannot
    /// overwrite a backup taken from a good config: once the repair succeeds the
    /// file parses and this is not called again.
    pub fn back_up_unreadable(cli_args: &CliArgs) -> Result<Option<PathBuf>, ConfigError> {
        let source = Self::config_file_path(cli_args);
        // A file that parses again (a login already saved over it) is not
        // the unreadable original; copying it would replace that backup.
        if !source.exists() || Self::read_from_file(cli_args).is_ok() {
            return Ok(None);
        }
        let backup = source.with_extension("toml.invalid");
        std::fs::copy(&source, &backup).map_err(ConfigError::WriteError)?;
        Ok(Some(backup))
    }

    /// Writes the configuration to a specific directory
    ///
    /// # Arguments
    /// * `config_dir` - Directory to write the config file to
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    fn write_to_file_at_dir(&self, config_dir: &Path) -> Result<(), ConfigError> {
        // Ensure directory exists and is writable
        Self::ensure_writable_directory(config_dir)?;

        // Get config file path and check permissions
        let config_file = config_dir.join(CONFIG_FILE);
        Self::ensure_writable_file(&config_file)?;

        // Serialize and write config atomically so access/refresh tokens never
        // land on disk as a partially-written pair.
        let config_str = toml::to_string(self).map_err(ConfigError::SerializeError)?;
        let temp_file = config_dir.join(format!(
            ".{CONFIG_FILE}.{}.{}.tmp",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            // Set the mode at creation so the tokens are never readable in a
            // window before a chmod.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&temp_file).map_err(ConfigError::WriteError)?;
            file.write_all(config_str.as_bytes())
                .map_err(ConfigError::WriteError)?;
            file.sync_all().map_err(ConfigError::WriteError)?;
        }
        std::fs::rename(&temp_file, &config_file).map_err(|error| {
            let _ = std::fs::remove_file(&temp_file);
            ConfigError::WriteError(error)
        })?;
        if let Some(parent) = config_file.parent()
            && let Ok(parent_file) = std::fs::File::open(parent)
        {
            let _ = parent_file.sync_all();
        }
        Ok(())
    }

    /// Ensures a directory exists and is writable
    ///
    /// # Arguments
    /// * `dir` - Directory to check
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    fn ensure_writable_directory(dir: &Path) -> Result<(), ConfigError> {
        if !dir.exists() {
            create_private_dir_all(dir).map_err(|e| {
                ConfigError::WriteError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("Failed to create config directory: {e}"),
                ))
            })?;
        }

        // Test write permissions by creating a temporary file
        let temp_file = dir.join(".pcl_test_write");
        std::fs::write(&temp_file, "").map_err(|e| {
            ConfigError::WriteError(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("No write permissions in config directory: {e}"),
            ))
        })?;
        std::fs::remove_file(&temp_file).ok(); // Clean up test file

        Ok(())
    }

    /// Ensures a file is writable
    ///
    /// # Arguments
    /// * `file` - File to check
    ///
    /// # Returns
    /// * `Result<(), ConfigError>` - Success or error
    fn ensure_writable_file(file: &PathBuf) -> Result<(), ConfigError> {
        if file.exists() {
            let metadata = std::fs::metadata(file).map_err(|e| {
                ConfigError::WriteError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("Failed to check file permissions: {e}"),
                ))
            })?;

            if metadata.permissions().readonly() {
                return Err(ConfigError::WriteError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Config file is read-only",
                )));
            }
        }
        Ok(())
    }

    /// Gets the legacy configuration directory path (~/.pcl)
    ///
    /// # Returns
    /// * `PathBuf` - Path to the legacy config directory
    ///
    /// # Panics
    ///
    /// Will panic if it does not find the home directory
    fn get_legacy_config_dir() -> PathBuf {
        home_dir().unwrap().join(LEGACY_CONFIG_DIR)
    }

    /// Gets the default configuration directory path
    ///
    /// Uses XDG Base Directory Specification:
    /// - `$XDG_CONFIG_HOME/pcl` if `XDG_CONFIG_HOME` is set
    /// - `~/.config/pcl` otherwise
    ///
    /// # Returns
    /// * `PathBuf` - Path to the config directory
    ///
    /// # Panics
    ///
    /// Will panic if it does not find the home directory
    pub fn get_config_dir() -> PathBuf {
        std::env::var("XDG_CONFIG_HOME")
            .map_or_else(|_| home_dir().unwrap().join(".config"), PathBuf::from)
            .join(CONFIG_DIR_NAME)
    }

    /// Migrates configuration from the legacy location (`~/.pcl`) to the new
    /// XDG-compliant location (`~/.config/pcl` or `$XDG_CONFIG_HOME/pcl`)
    ///
    /// Migration only occurs if:
    /// - The legacy directory exists
    /// - The new directory does not exist
    ///
    /// # Returns
    /// * `Ok(true)` - Migration was performed
    /// * `Ok(false)` - No migration needed
    /// * `Err(ConfigError)` - Migration failed
    pub fn migrate_legacy_config() -> Result<bool, ConfigError> {
        let legacy_dir = Self::get_legacy_config_dir();
        let new_dir = Self::get_config_dir();

        // Only migrate if legacy exists and new doesn't
        if legacy_dir.exists() && !new_dir.exists() {
            // Create parent dirs if needed
            if let Some(parent) = new_dir.parent() {
                std::fs::create_dir_all(parent).map_err(ConfigError::WriteError)?;
            }
            // Move the directory
            std::fs::rename(&legacy_dir, &new_dir).map_err(ConfigError::WriteError)?;
            if current_output_mode() == OutputMode::Human {
                eprintln!(
                    "{}: Migrated PCL config from {} to {}",
                    "Warning".yellow().bold(),
                    legacy_dir.display(),
                    new_dir.display()
                );
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Reads configuration from a specific directory
    ///
    /// # Arguments
    /// * `config_dir` - Directory to read the config file from
    ///
    /// # Returns
    /// * `Result<Self, ConfigError>` - Configuration or error
    fn read_from_file_at_dir(config_dir: &Path) -> Result<Self, ConfigError> {
        let config_file = config_dir.join(CONFIG_FILE);

        // If file doesn't exist, return default config
        if !config_file.exists() {
            return Ok(Self::default());
        }

        // Check if we have read permissions
        let metadata = std::fs::metadata(&config_file).map_err(|e| {
            ConfigError::ReadError(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                format!("Failed to check file permissions: {e}"),
            ))
        })?;

        if !metadata.permissions().readonly() {
            // Test read permissions
            std::fs::read_to_string(&config_file).map_err(|e| {
                ConfigError::ReadError(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("No read permissions for config file: {e}"),
                ))
            })?;
        }

        let config_str = std::fs::read_to_string(config_file).map_err(ConfigError::ReadError)?;
        toml::from_str(&config_str).map_err(ConfigError::ParseError)
    }

    /// Reads configuration from the default config file, or a specific directory
    ///
    /// If using the default config directory, this will first attempt to migrate
    /// any existing configuration from the legacy location (~/.pcl) to the new
    /// XDG-compliant location.
    ///
    /// # Arguments
    /// * `cli_args` - Command line arguments
    ///
    /// # Returns
    /// * `Result<Self, ConfigError>` - Configuration or error
    pub fn read_from_file(cli_args: &CliArgs) -> Result<Self, ConfigError> {
        // Only attempt migration when using default config dir
        if cli_args.config_dir.is_none() {
            // Attempt migration from legacy location (errors are non-fatal)
            let _ = Self::migrate_legacy_config();
        }

        Self::read_from_file_at_dir(
            &cli_args
                .config_dir
                .clone()
                .unwrap_or(Self::get_config_dir()),
        )
    }
}

fn config_show_envelope(config: &CliConfig, cli_args: &CliArgs) -> Value {
    with_envelope_metadata(json!({
        "status": "ok",
        "data": {
            "config_path": CliConfig::config_file_path(cli_args).display().to_string(),
            "platform_url": config.platform_url.as_deref(),
            "auth": config_auth_value(config),
            "rpc": redacted_rpc_endpoints(config),
        },
        "next_actions": if config.auth.is_some() {
            json!(["pcl auth status", "pcl account", "pcl doctor"])
        } else {
            json!(["pcl auth login", "pcl doctor"])
        },
    }))
}

fn config_auth_value(config: &CliConfig) -> Value {
    let Some(auth) = &config.auth else {
        return json!({
            "authenticated": false,
            "token_present": false,
            "refresh_token_present": false,
            "refresh_expires_at": null,
            "refresh_seconds_remaining": null,
            "token_valid": false,
            "token_expired": false,
            "expires_soon": false,
            "expired": false,
            "expires_at": null,
            "seconds_remaining": null,
            "expires_in_seconds": null,
        });
    };

    let now = Utc::now();
    let seconds_remaining = (auth.expires_at - now).num_seconds();
    let token_expired = auth.expires_at <= now;
    let expires_soon = !token_expired && seconds_remaining <= AUTH_EXPIRES_SOON_SECONDS;
    let refresh_seconds_remaining = auth
        .refresh_expires_at
        .map(|expires_at| (expires_at - now).num_seconds());
    json!({
        "authenticated": true,
        "user": auth.display_name(),
        "user_id": auth.user_id.map(|id| id.to_string()),
        "wallet_address": auth.wallet_address.map(|address| address.to_string()),
        "email": auth.email.as_deref(),
        "token_present": !auth.access_token.is_empty(),
        "refresh_token_present": !auth.refresh_token.is_empty(),
        "refresh_expires_at": auth.refresh_expires_at.map(|expires_at| expires_at.to_rfc3339()),
        "refresh_seconds_remaining": refresh_seconds_remaining,
        "token_valid": !token_expired,
        "token_expired": token_expired,
        "expires_soon": expires_soon,
        "expired": token_expired,
        "expires_at": auth.expires_at.to_rfc3339(),
        "seconds_remaining": seconds_remaining,
        "expires_in_seconds": seconds_remaining,
    })
}

fn print_config_output(value: &Value, json_output: bool) -> Result<(), ConfigError> {
    print!(
        "{}",
        envelope_output_string(value, json_output).map_err(ConfigError::JsonError)?
    );
    Ok(())
}

/// Non-secret view of the stored RPC endpoints for `pcl config show`.
/// Provider URLs commonly carry API keys in userinfo, path, or query
/// parameters, so only chain id, host, and confirmations are exposed; the
/// full URL never leaves config storage.
fn redacted_rpc_endpoints(config: &CliConfig) -> Value {
    let mut endpoints = serde_json::Map::new();
    for (chain_id, endpoint) in &config.rpc {
        endpoints.insert(
            chain_id.clone(),
            json!({
                "configured": true,
                "host": redacted_rpc_host(&endpoint.url),
                "confirmations": endpoint.confirmations,
            }),
        );
    }
    Value::Object(endpoints)
}

/// Scheme, host, and explicit port of an RPC URL; everything else (userinfo,
/// path, query) may carry credentials and is dropped.
pub(crate) fn redacted_rpc_host(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return "<unparseable-url>".to_string();
    };
    let host = parsed.host_str().unwrap_or("<unknown-host>");
    match parsed.port() {
        Some(port) => format!("{}://{host}:{port}", parsed.scheme()),
        None => format!("{}://{host}", parsed.scheme()),
    }
}

impl fmt::Display for CliConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let config_path = Self::get_config_dir().join(CONFIG_FILE);

        writeln!(f, "PCL Configuration")?;
        writeln!(f, "==================")?;
        writeln!(f, "Config path: {}", config_path.display())?;
        if let Some(platform_url) = &self.platform_url {
            writeln!(f, "Platform URL: {platform_url}")?;
        }

        match &self.auth {
            Some(auth) => writeln!(f, "{auth}")?,
            None => writeln!(f, "Authentication: Not authenticated")?,
        }

        if !self.rpc.is_empty() {
            writeln!(f, "RPC endpoints:")?;
            for (chain_id, endpoint) in &self.rpc {
                // Redacted: provider URLs commonly embed API keys.
                write!(
                    f,
                    "  chain {chain_id}: {}",
                    redacted_rpc_host(&endpoint.url)
                )?;
                match endpoint.confirmations {
                    Some(confirmations) => writeln!(f, " ({confirmations} confirmations)")?,
                    None => writeln!(f)?,
                }
            }
        }

        Ok(())
    }
}

/// Authentication details for a user
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAuth {
    /// Access token for API authentication
    pub access_token: String,
    /// Refresh token for obtaining new access tokens
    pub refresh_token: String,
    /// Token expiration timestamp
    #[serde(with = "chrono::serde::ts_seconds")]
    pub expires_at: DateTime<Utc>,
    /// Refresh token sliding expiration timestamp, when returned by the platform.
    #[serde(
        default,
        with = "chrono::serde::ts_seconds_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub refresh_expires_at: Option<DateTime<Utc>>,
    /// Platform user ID (UUID), used for API calls that require it
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<Uuid>,
    /// Ethereum address of the user (only present for wallet-based auth)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wallet_address: Option<Address>,
    /// Email address of the user (for email-based auth)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// The platform that issued these credentials.
    ///
    /// Deliberately stored *with* the credentials rather than alongside the
    /// remembered platform in [`CliConfig::platform_url`]. The two answer
    /// different questions — "which platform did I choose?" versus "who issued
    /// this token?" — and only the second may gate attaching the token to a
    /// request. Keeping provenance here makes that separation structural: it is
    /// written only when fresh credentials are stored, cleared with them on
    /// logout, and cannot be rebound by platform resolution or an interactive
    /// selection.
    ///
    /// `None` for credentials stored by a release that did not record it. There
    /// is no default to assume those into, so they belong to an unknown
    /// platform and force a one-time re-login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer_platform_url: Option<String>,
}

impl UserAuth {
    pub fn access_token_expires_at(&self) -> Option<DateTime<Utc>> {
        access_token_expires_at(&self.access_token)
    }

    /// Returns the best available display name for this user.
    pub fn display_name(&self) -> String {
        if let Some(addr) = &self.wallet_address
            && *addr != Address::ZERO
        {
            return addr.to_string();
        }
        if let Some(email) = &self.email {
            return email.clone();
        }
        if let Some(id) = &self.user_id {
            return id.to_string();
        }
        "unknown".to_string()
    }
}

pub fn access_token_expires_at(token: &str) -> Option<DateTime<Utc>> {
    let payload = token.split('.').nth(1)?;
    let payload = decode_base64_url(payload)?;
    let payload: Value = serde_json::from_slice(&payload).ok()?;
    let exp = payload.get("exp").and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|exp| i64::try_from(exp).ok()))
    })?;
    DateTime::from_timestamp(exp, 0)
}

fn decode_base64_url(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = 0u32;
    let mut bits = 0u8;

    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((buffer >> bits) & 0xff) as u8);
            buffer &= (1 << bits) - 1;
        }
    }

    Some(output)
}

impl fmt::Display for UserAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Authentication:")?;
        writeln!(f, "  User: {}", self.display_name())?;
        let now = Utc::now();
        let expired = self.expires_at < now;
        let expiry_text = self.expires_at.format("%Y-%m-%d %H:%M:%S UTC").to_string();
        if expired {
            writeln!(f, "  Token Expired at {}", expiry_text.red())?;
        } else {
            writeln!(f, "  Token Expires at {}", expiry_text.green())?;
        }

        // Don't display actual tokens for security reasons
        writeln!(f, "  Access Token: [Set]")?;
        writeln!(f, "  Refresh Token: [Set]")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        fs,
        io::Write,
        os::unix::fs::PermissionsExt,
    };
    use tempfile::TempDir;

    /// Helper function to set up a temporary config directory
    fn setup_config_dir() -> (PathBuf, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        unsafe {
            env::set_var("HOME", temp_dir.path());
            // Clear XDG_CONFIG_HOME to ensure we use ~/.config
            env::remove_var("XDG_CONFIG_HOME");
        }
        (
            temp_dir.path().join(".config").join(CONFIG_DIR_NAME),
            temp_dir,
        )
    }

    /// Helper function to create a read-only directory
    fn create_readonly_dir(path: &PathBuf) -> std::io::Result<()> {
        fs::create_dir_all(path)?;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o555); // Read and execute only
        fs::set_permissions(path, perms)
    }

    /// Helper function to create a read-only file
    fn create_readonly_file(path: &PathBuf) -> std::io::Result<()> {
        fs::write(path, "")?;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o444); // Read only
        fs::set_permissions(path, perms)
    }

    #[test]
    fn test_write_and_read_config() {
        let (config_dir, _temp_dir) = setup_config_dir();

        // Use a fixed timestamp for testing
        let fixed_timestamp = DateTime::from_timestamp(1672502400, 0).unwrap(); // 2022-12-31 16:00:00 UTC

        let config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "test_access".to_string(),
                refresh_token: "test_refresh".to_string(),
                expires_at: fixed_timestamp,
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };

        // Test writing
        config.write_to_file_at_dir(&config_dir).unwrap();
        assert_eq!(
            fs::metadata(config_dir.join(CONFIG_FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "the credentials file must be owner-only"
        );

        // Test reading
        let read_config = CliConfig::read_from_file_at_dir(&config_dir).unwrap();
        assert_eq!(
            read_config.auth.as_ref().unwrap().access_token,
            "test_access"
        );
        assert_eq!(
            read_config.auth.as_ref().unwrap().refresh_token,
            "test_refresh"
        );
        assert!(read_config.auth.as_ref().unwrap().wallet_address.is_none());

        // Test display format - check for key content rather than exact match due to color codes
        let formatted_cfg = format!("{read_config}");

        // Check that all the important information is present
        assert!(formatted_cfg.contains("PCL Configuration"));
        assert!(formatted_cfg.contains("Config path:"));
        assert!(formatted_cfg.contains("pcl/config.toml"));
        assert!(formatted_cfg.contains("User: unknown"));
        assert!(formatted_cfg.contains("2022-12-31 16:00:00 UTC"));
        assert!(formatted_cfg.contains("Access Token: [Set]"));
        assert!(formatted_cfg.contains("Refresh Token: [Set]"));
    }

    #[test]
    fn rpc_map_roundtrips_through_toml() {
        let mut config = CliConfig::default();
        config.rpc.insert(
            "84532".to_string(),
            RpcEndpoint {
                url: "https://sepolia.base.org".to_string(),
                confirmations: Some(3),
            },
        );
        config.rpc.insert(
            "31337".to_string(),
            RpcEndpoint {
                url: "http://127.0.0.1:8545".to_string(),
                confirmations: None,
            },
        );

        let serialized = toml::to_string(&config).unwrap();
        let deserialized: CliConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized, config);
        assert_eq!(
            deserialized.rpc_endpoint(84532).unwrap().confirmations,
            Some(3)
        );
        assert_eq!(
            deserialized.rpc_endpoint(31337).unwrap().url,
            "http://127.0.0.1:8545"
        );
        assert!(deserialized.rpc_endpoint(1).is_none());
    }

    #[test]
    fn legacy_config_without_rpc_section_still_parses() {
        let config: CliConfig = toml::from_str(
            r#"
[auth]
access_token = "a"
refresh_token = "r"
expires_at = 1672502400
"#,
        )
        .unwrap();
        assert!(config.rpc.is_empty());
        assert_eq!(config.auth.unwrap().access_token, "a");
    }

    #[test]
    fn empty_rpc_map_is_not_serialized() {
        let serialized = toml::to_string(&CliConfig::default()).unwrap();
        assert!(!serialized.contains("rpc"));
    }

    #[test]
    fn config_show_redacts_rpc_urls() {
        let mut config = CliConfig::default();
        config.rpc.insert(
            "84532".to_string(),
            RpcEndpoint {
                url: "https://user:hunter2@rpc.example.com/v2/apikey123?token=sekrit".to_string(),
                confirmations: Some(3),
            },
        );

        let envelope = config_show_envelope(&config, &CliArgs::default());
        let rendered = envelope.to_string();
        assert!(!rendered.contains("hunter2"));
        assert!(!rendered.contains("apikey123"));
        assert!(!rendered.contains("sekrit"));
        assert_eq!(
            envelope["data"]["rpc"]["84532"]["host"],
            "https://rpc.example.com"
        );
        assert_eq!(envelope["data"]["rpc"]["84532"]["configured"], true);
        assert_eq!(envelope["data"]["rpc"]["84532"]["confirmations"], 3);

        // The human formatter is redacted the same way.
        let display = config.to_string();
        assert!(display.contains("https://rpc.example.com"));
        assert!(!display.contains("hunter2"));
        assert!(!display.contains("apikey123"));
    }

    #[test]
    fn redacted_rpc_host_keeps_scheme_host_and_port() {
        assert_eq!(
            redacted_rpc_host("http://127.0.0.1:8545"),
            "http://127.0.0.1:8545"
        );
        assert_eq!(
            redacted_rpc_host("https://mainnet.infura.io/v3/SECRET"),
            "https://mainnet.infura.io"
        );
        assert_eq!(redacted_rpc_host("not a url"), "<unparseable-url>");
    }

    /// A config carrying credentials, as another process would have left it.
    fn authenticated_config(access: &str, refresh: &str) -> CliConfig {
        CliConfig {
            auth: Some(UserAuth {
                access_token: access.to_string(),
                refresh_token: refresh.to_string(),
                expires_at: DateTime::from_timestamp(4_102_444_800, 0).expect("timestamp"),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
                issuer_platform_url: Some("https://linea.phylax.systems".to_string()),
            }),
            ..CliConfig::default()
        }
    }

    /// Recording a selected platform must not roll back an update that landed
    /// before the merge started.
    ///
    /// The interactive selector can sit open indefinitely, and another `pcl` may
    /// rotate the refresh token while it does. Writing this process's pre-prompt
    /// snapshot would discard the rotation and leave a refresh token the
    /// platform has already invalidated.
    #[tokio::test]
    async fn merging_a_selected_platform_keeps_an_already_rotated_token() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };

        let stale = authenticated_config("old-access", "old-refresh");
        authenticated_config("rotated-access", "rotated-refresh")
            .write_to_file(&cli_args)
            .expect("another process writes the rotated pair");

        let mut config = stale.clone();
        config.platform_url = Some("https://ethereum.phylax.systems".to_string());
        config
            .merge_selected_platform(&cli_args)
            .await
            .expect("merge the selection");

        let on_disk = CliConfig::read_from_file(&cli_args).expect("read merged config");
        let auth = on_disk.auth.as_ref().expect("credentials survive");
        assert_eq!(
            auth.refresh_token, "rotated-refresh",
            "the concurrently rotated refresh token must survive"
        );
        assert_eq!(auth.access_token, "rotated-access");
        assert_eq!(
            on_disk.platform_url.as_deref(),
            Some("https://ethereum.phylax.systems"),
            "the selection still has to be recorded"
        );
        assert_eq!(
            config, on_disk,
            "this process adopts the merged state for the rest of the run"
        );
    }

    /// The rotation that overlaps the merge is the dangerous one.
    ///
    /// Reloading before the write only protects updates that finished before the
    /// reload. Without a lock, a refresh that writes its new pair *after* the
    /// reload and before the merge's own write is silently overwritten, throwing
    /// away the only valid refresh token — and the sibling test above cannot
    /// catch it, because there the rotation is already on disk when the merge
    /// begins.
    ///
    /// The lock itself is the barrier here: holding it forces the merge to wait,
    /// and the rotation is written while it waits, so the rotation lands strictly
    /// between the merge being asked for and the merge reading the file.
    #[tokio::test]
    async fn merging_a_selected_platform_waits_for_an_overlapping_rotation() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };

        let stale = authenticated_config("old-access", "old-refresh");
        stale
            .write_to_file(&cli_args)
            .expect("the pre-prompt state is on disk");

        // Stand in for a concurrent `refresh_stored_auth`, which holds this same
        // lock across its own read-modify-write.
        let lock = ConfigLock::acquire(&cli_args)
            .await
            .expect("the refresher takes the lock first");

        let mut config = stale.clone();
        config.platform_url = Some("https://ethereum.phylax.systems".to_string());
        let merge_args = cli_args.clone();
        let merge = tokio::spawn(async move {
            config.merge_selected_platform(&merge_args).await?;
            Ok::<CliConfig, ConfigError>(config)
        });

        // The merge cannot have read anything yet: it is parked on the lock.
        tokio::time::sleep(CONFIG_LOCK_POLL_INTERVAL * 3).await;
        assert!(
            !merge.is_finished(),
            "the merge must block until the refresher releases the lock"
        );
        authenticated_config("rotated-access", "rotated-refresh")
            .write_to_file(&cli_args)
            .expect("the refresher writes its rotated pair");
        drop(lock);

        let merged = merge
            .await
            .expect("merge task joins")
            .expect("merge the selection");

        let on_disk = CliConfig::read_from_file(&cli_args).expect("read merged config");
        let auth = on_disk.auth.as_ref().expect("credentials survive");
        assert_eq!(
            auth.refresh_token, "rotated-refresh",
            "a rotation overlapping the merge must survive it"
        );
        assert_eq!(auth.access_token, "rotated-access");
        assert_eq!(
            on_disk.platform_url.as_deref(),
            Some("https://ethereum.phylax.systems"),
            "the selection still has to be recorded"
        );
        assert_eq!(merged, on_disk);
    }

    /// A lock another live process still holds must not wait forever.
    ///
    /// Runs on a paused clock, which tokio auto-advances whenever every task is
    /// parked on a timer, so the whole `CONFIG_LOCK_TIMEOUT` budget elapses
    /// instantly instead of costing the suite 30 real seconds.
    #[tokio::test(start_paused = true)]
    async fn a_held_config_lock_times_out_rather_than_blocking_forever() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let _held = ConfigLock::acquire(&cli_args).await.expect("first acquire");

        let error = ConfigLock::acquire(&cli_args)
            .await
            .expect_err("the lock is still held, so acquiring must fail");
        assert!(
            matches!(error, ConfigError::LockTimeout),
            "expected a lock timeout, got {error:?}"
        );
    }

    /// The previous holder is gone when its lock file is still on disk and no
    /// process has the OS lock. That is what a killed process leaves behind.
    #[tokio::test]
    async fn a_lock_file_with_no_live_holder_is_free() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let path = CliConfig::config_file_path(&cli_args).with_extension("toml.lock");
        let pid = dead_pid();
        std::fs::write(&path, format!("pid={pid} acquired_at=gone\n")).expect("leave a lock file");

        let acquired = tokio::time::timeout(Duration::from_secs(2), ConfigLock::acquire(&cli_args))
            .await
            .expect("a leftover lock file is not a live lock");
        acquired.expect("acquire");
    }

    /// Killing the holder must free the lock without waiting out the timeout.
    /// The child takes the same OS lock and is then killed, so `Drop` never runs.
    #[tokio::test]
    async fn a_killed_holder_releases_the_config_lock() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let ready = config_dir.path().join("ready");
        let mut child =
            std::process::Command::new(std::env::current_exe().expect("test executable"))
                .arg("--exact")
                .arg("config::tests::a_config_lock_holder_process")
                .env("PCL_HOLD_CONFIG_LOCK", config_dir.path())
                .env("PCL_HOLD_CONFIG_LOCK_READY", &ready)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn holder");
        let started = std::time::Instant::now();
        while !ready.exists() {
            if started.elapsed() > std::time::Duration::from_secs(15) {
                let _ = child.kill();
                panic!("holder never acquired the lock");
            }
            if let Some(status) = child.try_wait().expect("holder status") {
                panic!("holder exited before acquiring the lock: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        child.kill().expect("kill holder");
        child.wait().expect("wait for holder");

        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let acquired = tokio::time::timeout(Duration::from_secs(2), ConfigLock::acquire(&cli_args))
            .await
            .expect("a killed holder must not keep the lock");
        acquired.expect("acquire after the holder is gone");
    }

    /// A symlink at the lock path must not be followed into its target.
    #[tokio::test]
    async fn a_lock_symlink_does_not_truncate_its_target() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let target = config_dir.path().join("precious");
        std::fs::write(&target, b"keep-me").expect("precious file");
        let path = CliConfig::config_file_path(&cli_args).with_extension("toml.lock");
        std::os::unix::fs::symlink(&target, &path).expect("plant a symlink at the lock path");

        let acquired = ConfigLock::acquire(&cli_args)
            .await
            .expect("a planted symlink is not the lock file");
        assert_eq!(
            std::fs::read(&target).expect("target still readable"),
            b"keep-me"
        );
        assert!(
            std::fs::symlink_metadata(&path)
                .expect("lock path")
                .file_type()
                .is_file()
        );
        drop(acquired);
        assert_eq!(
            std::fs::read(&target).expect("target still readable"),
            b"keep-me"
        );
    }

    /// An older CLI holds the lock by the file's presence and a live pid, and
    /// has already closed the descriptor. A newer CLI must wait. After the
    /// older writer removes the file, the newer one enters, and its own drop
    /// removes the file so the older `create_new` can succeed.
    #[tokio::test]
    async fn an_old_presence_lock_blocks_a_new_writer_until_it_is_released() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let path = CliConfig::config_file_path(&cli_args).with_extension("toml.lock");
        let mut old = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("older CLI creates the lock");
        writeln!(old, "pid={} acquired_at=old", std::process::id())
            .expect("older CLI records its pid");
        old.sync_all().expect("older CLI syncs the lock");
        drop(old);

        let blocked =
            tokio::time::timeout(Duration::from_millis(400), ConfigLock::acquire(&cli_args)).await;
        assert!(
            blocked.is_err(),
            "a newer CLI entered while an older writer still held the presence lock"
        );

        std::fs::remove_file(&path).expect("older CLI drops the lock");
        let acquired = tokio::time::timeout(Duration::from_secs(2), ConfigLock::acquire(&cli_args))
            .await
            .expect("the presence lock is gone")
            .expect("acquire");
        drop(acquired);

        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("an older CLI creates the lock after a newer one exits");
    }

    fn dead_pid() -> u32 {
        let mut pid = 1_000_000u32;
        while process_is_alive(pid) && pid < 1_000_050 {
            pid += 1;
        }
        pid
    }

    fn lock_identity(path: &Path) -> (u64, u64) {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(path).expect("lock metadata");
        (meta.dev(), meta.ino())
    }

    fn spawn_holder(
        config_dir: &Path,
        ready: &Path,
        extra_env: &[(&str, &Path)],
    ) -> std::process::Child {
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("test executable"));
        command
            .arg("--exact")
            .arg("config::tests::a_config_lock_holder_process")
            .env("PCL_HOLD_CONFIG_LOCK", config_dir)
            .env("PCL_HOLD_CONFIG_LOCK_READY", ready)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        command.spawn().expect("spawn holder")
    }

    fn wait_for_file(child: &mut std::process::Child, path: &Path, what: &str) {
        let started = std::time::Instant::now();
        while !path.exists() {
            if started.elapsed() > std::time::Duration::from_secs(15) {
                let _ = child.kill();
                panic!("{what} did not appear");
            }
            if let Some(status) = child.try_wait().expect("holder status") {
                panic!("holder exited before {what}: {status}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    async fn acquired_within(config_dir: &Path, budget: Duration) -> bool {
        let cli_args = CliArgs {
            config_dir: Some(config_dir.to_path_buf()),
            ..CliArgs::default()
        };
        tokio::time::timeout(budget, ConfigLock::acquire(&cli_args))
            .await
            .is_ok_and(|result| result.is_ok())
    }

    /// Two reapers and a new writer share a killed holder's file. The reaper
    /// that holds the advisory lock unlinks it; the others must not delete the
    /// replacement that reaper then creates.
    #[tokio::test]
    async fn stale_reapers_do_not_unlink_a_live_replacement() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let path = CliConfig::config_file_path(&cli_args).with_extension("toml.lock");
        let pid = dead_pid();
        std::fs::write(&path, format!("pid={pid} acquired_at=gone\n")).expect("stale lock");
        let stale_id = lock_identity(&path);

        let paused = config_dir.path().join("reap-paused");
        let release = config_dir.path().join("reap-release");
        let ready = config_dir.path().join("ready");
        let mut child = spawn_holder(
            config_dir.path(),
            &ready,
            &[
                ("PCL_LOCK_BEFORE_REAP_UNLINK", paused.as_path()),
                ("PCL_LOCK_BEFORE_REAP_UNLINK_RELEASE", release.as_path()),
            ],
        );
        wait_for_file(&mut child, &paused, "reaper pause");

        let dir = config_dir.path().to_path_buf();
        let competing_reaper = tokio::spawn({
            let dir = dir.clone();
            async move { acquired_within(&dir, Duration::from_millis(400)).await }
        });
        let new_writer =
            tokio::spawn(async move { acquired_within(&dir, Duration::from_millis(400)).await });
        let (competing_reaper, new_writer) = tokio::join!(competing_reaper, new_writer);
        assert!(
            !competing_reaper.expect("competing reaper") && !new_writer.expect("new writer"),
            "a contender acquired the lock while the stale inode was still held"
        );
        assert_eq!(
            lock_identity(&path),
            stale_id,
            "a contender replaced the stale lock before its holder unlinked it"
        );
        let stale_body = std::fs::read_to_string(&path).expect("stale lock body");
        assert!(
            stale_body.starts_with(&format!("pid={pid} ")),
            "a contender rewrote the stale lock: {stale_body}"
        );

        std::fs::write(&release, "go").expect("release the reaper");
        wait_for_file(&mut child, &ready, "reaper acquire");
        let replacement = std::fs::read_to_string(&path).expect("replacement lock");
        assert!(
            replacement.starts_with(&format!("pid={} ", child.id())),
            "the reaper should replace the stale file with its own lock, got {replacement}"
        );
        assert!(
            !acquired_within(config_dir.path(), Duration::from_millis(300)).await,
            "a writer entered while the reaper held the replacement"
        );

        child.kill().expect("kill reaper");
        child.wait().expect("wait for reaper");
        assert!(
            acquired_within(config_dir.path(), Duration::from_secs(2)).await,
            "the replacement is free after its holder is gone"
        );
    }

    /// A contender's probe takes the advisory lock after the creator has
    /// written its pid and before the creator locks. The creator must keep
    /// its file and finish once the probe drops.
    #[tokio::test]
    async fn a_probe_does_not_abort_the_lock_creator() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let paused = config_dir.path().join("creator-paused");
        let release = config_dir.path().join("creator-release");
        let ready = config_dir.path().join("ready");
        let mut child = spawn_holder(
            config_dir.path(),
            &ready,
            &[
                ("PCL_LOCK_BEFORE_FLOCK", paused.as_path()),
                ("PCL_LOCK_BEFORE_FLOCK_RELEASE", release.as_path()),
            ],
        );
        wait_for_file(&mut child, &paused, "creator pause");

        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let path = CliConfig::config_file_path(&cli_args).with_extension("toml.lock");
        let probe = open_nofollow_read(&path).expect("open the creator's lock");
        try_lock_exclusive(&probe).expect("probe the creator before it locks");
        let body = std::fs::read(&path).expect("creator wrote its pid");
        std::fs::write(&release, "go").expect("release the creator");
        std::thread::sleep(std::time::Duration::from_millis(250));

        assert!(
            child.try_wait().expect("creator status").is_none(),
            "the creator exited while the probe held its lock"
        );
        assert!(
            !ready.exists(),
            "the creator finished while the probe held its lock"
        );
        assert_eq!(
            std::fs::read(&path).expect("creator file still present"),
            body,
            "the creator unlinked its own lock after the probe"
        );

        drop(probe);
        wait_for_file(&mut child, &ready, "creator acquire");
        assert!(
            !acquired_within(config_dir.path(), Duration::from_millis(300)).await,
            "a contender entered while the creator held the lock"
        );
        child.kill().expect("kill creator");
        child.wait().expect("wait for creator");
    }

    /// The waiter opens the holder's lock, then the holder drops it before the
    /// identity check. Acquisition retries and takes the path.
    #[tokio::test]
    async fn a_waiter_retries_when_the_holder_unlinks_after_open() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let holder = ConfigLock::acquire(&cli_args)
            .await
            .expect("holder acquires");
        let paused = config_dir.path().join("open-paused");
        let release = config_dir.path().join("open-release");
        let ready = config_dir.path().join("ready");
        let mut child = spawn_holder(
            config_dir.path(),
            &ready,
            &[
                ("PCL_LOCK_AFTER_OPEN", paused.as_path()),
                ("PCL_LOCK_AFTER_OPEN_RELEASE", release.as_path()),
            ],
        );
        wait_for_file(&mut child, &paused, "waiter pause");
        drop(holder);
        std::fs::write(&release, "go").expect("release the waiter");
        wait_for_file(&mut child, &ready, "waiter acquire");
        assert!(
            !acquired_within(config_dir.path(), Duration::from_millis(300)).await,
            "a second writer entered while the waiter held the lock it retried into"
        );
        child.kill().expect("kill waiter");
        child.wait().expect("wait for waiter");
    }

    /// The waiter opens the holder's lock, then another writer replaces it
    /// before the identity check. The replacement is not deleted, and the
    /// waiter acquires only after that writer drops it.
    #[tokio::test]
    async fn a_waiter_retries_when_the_holder_is_replaced_after_open() {
        let config_dir = tempfile::tempdir().expect("temp config dir");
        let cli_args = CliArgs {
            config_dir: Some(config_dir.path().to_path_buf()),
            ..CliArgs::default()
        };
        let path = CliConfig::config_file_path(&cli_args).with_extension("toml.lock");
        let holder = ConfigLock::acquire(&cli_args)
            .await
            .expect("holder acquires");
        let paused = config_dir.path().join("open-paused");
        let release = config_dir.path().join("open-release");
        let ready = config_dir.path().join("ready");
        let mut child = spawn_holder(
            config_dir.path(),
            &ready,
            &[
                ("PCL_LOCK_AFTER_OPEN", paused.as_path()),
                ("PCL_LOCK_AFTER_OPEN_RELEASE", release.as_path()),
            ],
        );
        wait_for_file(&mut child, &paused, "waiter pause");
        drop(holder);
        let replacement = ConfigLock::acquire(&cli_args)
            .await
            .expect("replacement writer acquires");
        let replacement_body = std::fs::read(&path).expect("replacement lock");
        let replacement_id = lock_identity(&path);
        std::fs::write(&release, "go").expect("release the waiter");
        std::thread::sleep(std::time::Duration::from_millis(250));
        assert!(
            child.try_wait().expect("waiter status").is_none(),
            "the waiter aborted after the lock was replaced"
        );
        assert!(
            !ready.exists(),
            "the waiter took the replacement writer's lock"
        );
        assert_eq!(
            std::fs::read(&path).expect("replacement still present"),
            replacement_body,
            "the waiter unlinked the replacement"
        );
        assert_eq!(lock_identity(&path), replacement_id);
        drop(replacement);
        wait_for_file(&mut child, &ready, "waiter acquire after replacement");
        child.kill().expect("kill waiter");
        child.wait().expect("wait for waiter");
    }

    /// Child process for [`a_killed_holder_releases_the_config_lock`].
    /// Without `PCL_HOLD_CONFIG_LOCK` it returns immediately.
    #[tokio::test]
    async fn a_config_lock_holder_process() {
        let Ok(dir) = std::env::var("PCL_HOLD_CONFIG_LOCK") else {
            return;
        };
        let ready = std::env::var("PCL_HOLD_CONFIG_LOCK_READY").expect("ready path");
        let cli_args = CliArgs {
            config_dir: Some(PathBuf::from(dir)),
            ..CliArgs::default()
        };
        let _lock = ConfigLock::acquire(&cli_args)
            .await
            .expect("hold the config lock");
        std::fs::write(ready, "held").expect("signal that the lock is held");
        std::future::pending::<()>().await;
    }

    #[test]
    fn set_rpc_rejects_invalid_urls() {
        let mut config = CliConfig::default();
        let args = ConfigArgs {
            command: ConfigCommand::SetRpc {
                chain_id: 1,
                url: "not a url".to_string(),
                confirmations: None,
            },
        };
        assert!(args.run(&mut config, &CliArgs::default()).is_err());

        let args = ConfigArgs {
            command: ConfigCommand::SetRpc {
                chain_id: 1,
                url: "ws://example.com".to_string(),
                confirmations: None,
            },
        };
        assert!(args.run(&mut config, &CliArgs::default()).is_err());
        assert!(config.rpc.is_empty());
    }

    #[test]
    fn set_rpc_parse_errors_do_not_expose_url_credentials() {
        let mut config = CliConfig::default();
        let args = ConfigArgs {
            command: ConfigCommand::SetRpc {
                chain_id: 1,
                url: "https://user:hunter2@rpc.example.com:invalid/v2/apikey123?token=sekrit"
                    .to_string(),
                confirmations: None,
            },
        };

        let error = args
            .run(&mut config, &CliArgs::default())
            .unwrap_err()
            .to_string();

        assert!(error.contains("<unparseable-url>"));
        for secret in ["user", "hunter2", "apikey123", "sekrit"] {
            assert!(!error.contains(secret), "error exposed {secret:?}: {error}");
        }
        assert!(config.rpc.is_empty());
    }

    #[test]
    fn set_and_unset_rpc_mutate_config() {
        let mut config = CliConfig::default();
        let args = ConfigArgs {
            command: ConfigCommand::SetRpc {
                chain_id: 84532,
                url: "https://sepolia.base.org".to_string(),
                confirmations: Some(2),
            },
        };
        args.run(&mut config, &CliArgs::default()).unwrap();
        assert_eq!(
            config.rpc_endpoint(84532),
            Some(&RpcEndpoint {
                url: "https://sepolia.base.org".to_string(),
                confirmations: Some(2),
            })
        );

        let args = ConfigArgs {
            command: ConfigCommand::UnsetRpc { chain_id: 84532 },
        };
        args.run(&mut config, &CliArgs::default()).unwrap();
        assert!(config.rpc.is_empty());
    }

    #[test]
    fn write_to_file_if_unchanged_preserves_newer_disk_auth() {
        let temp_dir = TempDir::new().unwrap();
        let cli_args = CliArgs {
            config_dir: Some(temp_dir.path().to_path_buf()),
            ..Default::default()
        };
        let old_config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "old_access".to_string(),
                refresh_token: "old_refresh".to_string(),
                expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };
        let stale_process_config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "normalized_old_access".to_string(),
                refresh_token: "old_refresh".to_string(),
                expires_at: DateTime::from_timestamp(4102444800, 0).unwrap(),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };
        let newer_config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "new_access".to_string(),
                refresh_token: "new_refresh".to_string(),
                expires_at: DateTime::from_timestamp(4102444800, 0).unwrap(),
                refresh_expires_at: Some(DateTime::from_timestamp(4105036800, 0).unwrap()),
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };

        old_config.write_to_file(&cli_args).unwrap();
        newer_config.write_to_file(&cli_args).unwrap();

        let wrote = stale_process_config
            .write_to_file_if_unchanged(&cli_args, &old_config)
            .unwrap();

        assert!(!wrote);
        let persisted = CliConfig::read_from_file(&cli_args).unwrap();
        assert_eq!(
            persisted.auth.as_ref().unwrap().refresh_token,
            "new_refresh"
        );
    }

    #[test]
    fn platform_url_round_trips_and_is_omitted_when_unset() {
        let (config_dir, _temp_dir) = setup_config_dir();

        let config = CliConfig {
            auth: None,
            platform_url: Some("https://custom.phylax.example".to_string()),
            rpc: BTreeMap::default(),
        };
        config.write_to_file_at_dir(&config_dir).unwrap();
        let read_config = CliConfig::read_from_file_at_dir(&config_dir).unwrap();
        assert_eq!(
            read_config.platform_url.as_deref(),
            Some("https://custom.phylax.example")
        );

        CliConfig::default()
            .write_to_file_at_dir(&config_dir)
            .unwrap();
        let raw = fs::read_to_string(config_dir.join(CONFIG_FILE)).unwrap();
        assert!(!raw.contains("platform_url"));
    }

    #[test]
    fn config_without_platform_url_parses_as_none() {
        let (config_dir, _temp_dir) = setup_config_dir();
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_dir.join(CONFIG_FILE), "").unwrap();

        let config = CliConfig::read_from_file_at_dir(&config_dir).unwrap();
        assert!(config.platform_url.is_none());
    }

    #[test]
    fn test_read_nonexistent_config() {
        let (config_dir, _temp_dir) = setup_config_dir();

        // Try reading without creating a file
        let config = CliConfig::read_from_file_at_dir(&config_dir).unwrap();
        assert!(config.auth.is_none());
    }

    #[test]
    fn test_user_auth_display() {
        let auth = UserAuth {
            issuer_platform_url: None,
            access_token: "test_access".to_string(),
            refresh_token: "test_refresh".to_string(),
            expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(), // 2022-12-31 16:00:00 UTC
            refresh_expires_at: None,
            user_id: None,
            wallet_address: None,
            email: Some("test@example.com".to_string()),
        };

        let display = format!("{auth}");
        assert!(display.contains("User: test@example.com"));
        assert!(display.contains("Token Expired at"));
        assert!(display.contains("Access Token: [Set]"));
        assert!(display.contains("Refresh Token: [Set]"));
    }

    #[test]
    fn test_display_name_priority() {
        let expires = DateTime::from_timestamp(0, 0).unwrap();

        // Non-zero wallet address takes priority over everything
        let with_addr = UserAuth {
            issuer_platform_url: None,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: expires,
            refresh_expires_at: None,
            wallet_address: Some(Address::from_slice(&[1; 20])),
            email: Some("test@example.com".to_string()),
            user_id: Some(Uuid::nil()),
        };
        assert_eq!(
            with_addr.display_name(),
            "0x0101010101010101010101010101010101010101"
        );

        // Email is next priority when no wallet address
        let with_email = UserAuth {
            issuer_platform_url: None,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: expires,
            refresh_expires_at: None,
            wallet_address: None,
            email: Some("test@example.com".to_string()),
            user_id: Some(Uuid::nil()),
        };
        assert_eq!(with_email.display_name(), "test@example.com");

        // User ID is fallback when no address or email
        let with_id = UserAuth {
            issuer_platform_url: None,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: expires,
            refresh_expires_at: None,
            wallet_address: None,
            email: None,
            user_id: Some(Uuid::nil()),
        };
        assert_eq!(
            with_id.display_name(),
            "00000000-0000-0000-0000-000000000000"
        );

        // "unknown" when nothing is set
        let bare = UserAuth {
            issuer_platform_url: None,
            access_token: String::new(),
            refresh_token: String::new(),
            expires_at: expires,
            refresh_expires_at: None,
            wallet_address: None,
            email: None,
            user_id: None,
        };
        assert_eq!(bare.display_name(), "unknown");
    }

    #[test]
    fn extracts_expiry_from_jwt_access_token() {
        let auth = UserAuth {
            issuer_platform_url: None,
            access_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string(),
            refresh_token: String::new(),
            expires_at: DateTime::from_timestamp(0, 0).unwrap(),
            refresh_expires_at: None,
            wallet_address: None,
            email: None,
            user_id: None,
        };

        assert_eq!(
            auth.access_token_expires_at(),
            DateTime::from_timestamp(4102444800, 0)
        );
    }

    #[test]
    fn normalizes_legacy_device_session_expiry_from_access_token_exp() {
        let mut config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "e30.eyJleHAiOjQxMDI0NDQ4MDB9.sig".to_string(),
                refresh_token: "refresh".to_string(),
                expires_at: DateTime::from_timestamp(1, 0).unwrap(),
                refresh_expires_at: None,
                wallet_address: None,
                email: None,
                user_id: None,
            }),
            platform_url: None,
        };

        assert!(config.normalize_auth_expiry_from_access_token());
        assert_eq!(
            config.auth.unwrap().expires_at,
            DateTime::from_timestamp(4102444800, 0).unwrap()
        );
    }

    #[test]
    fn test_config_args_show() {
        let mut config = CliConfig::default();
        let args = ConfigArgs {
            command: ConfigCommand::Show,
        };
        assert!(args.run(&mut config, &CliArgs::default()).is_ok());
    }

    #[test]
    fn test_config_args_delete() {
        let mut config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "test".to_string(),
                refresh_token: "test".to_string(),
                expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
                refresh_expires_at: None,
                user_id: None,
                wallet_address: None,
                email: None,
            }),
            platform_url: None,
        };
        let args = ConfigArgs {
            command: ConfigCommand::Delete,
        };
        assert!(args.run(&mut config, &CliArgs::default()).is_ok());
        assert!(config.auth.is_none());
    }

    #[test]
    fn config_show_envelope_hides_tokens_and_reports_expiry() {
        let args = CliArgs {
            config_dir: Some(PathBuf::from("/tmp/pcl-test-config")),
            ..Default::default()
        };
        let config = CliConfig {
            rpc: BTreeMap::default(),
            auth: Some(UserAuth {
                issuer_platform_url: None,
                access_token: "secret-access".to_string(),
                refresh_token: "secret-refresh".to_string(),
                expires_at: Utc::now() + chrono::Duration::minutes(10),
                refresh_expires_at: None,
                user_id: Some(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()),
                wallet_address: None,
                email: Some("test@example.com".to_string()),
            }),
            platform_url: None,
        };

        let envelope = config_show_envelope(&config, &args);

        assert_eq!(envelope["status"], "ok");
        assert_eq!(envelope["schema_version"], "pcl.envelope.v1");
        assert_eq!(envelope["pcl_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            envelope["data"]["config_path"],
            "/tmp/pcl-test-config/config.toml"
        );
        assert_eq!(envelope["data"]["auth"]["authenticated"], true);
        assert_eq!(envelope["data"]["auth"]["user"], "test@example.com");
        assert_eq!(envelope["data"]["auth"]["token_valid"], true);
        assert_eq!(envelope["data"]["auth"]["expired"], false);
        assert!(envelope["data"]["auth"]["seconds_remaining"].is_number());
        let serialized = serde_json::to_string(&envelope).unwrap();
        assert!(!serialized.contains("secret-access"));
        assert!(!serialized.contains("secret-refresh"));
    }

    #[test]
    fn test_write_to_file_permission_error() {
        let temp_dir = tempfile::tempdir().unwrap();

        // Create a read-only directory
        let mut perms = std::fs::metadata(&temp_dir).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&temp_dir, perms).unwrap();

        let config = CliConfig::default();
        let result = config.write_to_file_at_dir(temp_dir.path());

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Permission denied")
        );
    }

    #[test]
    fn test_read_from_file_invalid_toml() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(config_file, "invalid toml content").unwrap();

        let result = CliConfig::read_from_file_at_dir(&config_dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_display_empty_config() {
        let config = CliConfig::default();
        let display = format!("{config}");
        assert!(display.contains("Not authenticated"));
    }

    #[test]
    fn test_user_auth_serialization() {
        let auth = UserAuth {
            issuer_platform_url: None,
            access_token: "test_access".to_string(),
            refresh_token: "test_refresh".to_string(),
            expires_at: DateTime::from_timestamp(1672502400, 0).unwrap(),
            refresh_expires_at: Some(DateTime::from_timestamp(1675094400, 0).unwrap()),
            user_id: Some(Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()),
            wallet_address: Some(Address::from_slice(&[0; 20])),
            email: Some("test@example.com".to_string()),
        };

        let serialized = toml::to_string(&auth).unwrap();
        let deserialized: UserAuth = toml::from_str(&serialized).unwrap();

        assert_eq!(auth.access_token, deserialized.access_token);
        assert_eq!(auth.refresh_token, deserialized.refresh_token);
        assert_eq!(auth.wallet_address, deserialized.wallet_address);
        assert_eq!(auth.expires_at, deserialized.expires_at);
    }

    #[test]
    fn test_ensure_writable_directory_success() {
        let (config_dir, _temp_dir) = setup_config_dir();
        assert!(CliConfig::ensure_writable_directory(&config_dir).is_ok());
    }

    #[test]
    fn test_ensure_writable_directory_readonly() {
        let (config_dir, _temp_dir) = setup_config_dir();
        create_readonly_dir(&config_dir).unwrap();

        let result = CliConfig::ensure_writable_directory(&config_dir);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Permission denied")
        );
    }

    #[test]
    fn test_ensure_writable_file_success() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(&config_file, "").unwrap();

        assert!(CliConfig::ensure_writable_file(&config_file).is_ok());
    }

    #[test]
    fn test_ensure_writable_file_readonly() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);
        fs::create_dir_all(&config_dir).unwrap();
        create_readonly_file(&config_file).unwrap();

        let result = CliConfig::ensure_writable_file(&config_file);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("read-only"));
    }

    #[test]
    fn test_ensure_writable_file_nonexistent() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);

        assert!(CliConfig::ensure_writable_file(&config_file).is_ok());
    }

    #[test]
    fn test_write_to_file_at_dir_permission_denied() {
        let (config_dir, _temp_dir) = setup_config_dir();
        create_readonly_dir(&config_dir).unwrap();

        let config = CliConfig::default();
        let result = config.write_to_file_at_dir(&config_dir);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Permission denied")
        );
    }

    #[test]
    fn test_write_to_file_at_dir_readonly_file() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config_file = config_dir.join(CONFIG_FILE);
        fs::create_dir_all(&config_dir).unwrap();
        create_readonly_file(&config_file).unwrap();

        let config = CliConfig::default();
        let result = config.write_to_file_at_dir(&config_dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("read-only"));
    }

    #[test]
    fn test_write_to_file_at_dir_success() {
        let (config_dir, _temp_dir) = setup_config_dir();
        let config = CliConfig::default();
        assert!(config.write_to_file_at_dir(&config_dir).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn write_creates_a_missing_config_dir_owner_only() {
        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().join("new").join("pcl");
        CliConfig::default()
            .write_to_file_at_dir(&config_dir)
            .unwrap();
        assert_eq!(
            fs::metadata(&config_dir).unwrap().permissions().mode() & 0o777,
            0o700,
            "a config dir created for credentials must be 0700"
        );

        // An existing user dir keeps its mode.
        let existing = TempDir::new().unwrap();
        fs::set_permissions(existing.path(), fs::Permissions::from_mode(0o755)).unwrap();
        CliConfig::default()
            .write_to_file_at_dir(existing.path())
            .unwrap();
        assert_eq!(
            fs::metadata(existing.path()).unwrap().permissions().mode() & 0o777,
            0o755,
            "an existing config dir must not be chmodded"
        );
    }
}
