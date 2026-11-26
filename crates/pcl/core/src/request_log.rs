use crate::config::CliConfig;
use pcl_common::args::CliArgs;
use serde_json::Value;
use std::{
    collections::VecDeque,
    env,
    fs::{
        self,
        OpenOptions,
    },
    io::{
        BufRead,
        BufReader,
        Seek,
        SeekFrom,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
};

#[cfg(unix)]
use std::os::unix::fs::{
    DirBuilderExt,
    OpenOptionsExt,
    PermissionsExt,
};

const REQUEST_LOG_ENV: &str = "PCL_REQUEST_LOG";
const REQUEST_LOG_FILE: &str = "requests.jsonl";
const REDACTED_SEGMENT: &str = "<redacted>";
const MIGRATION_TEMP_PREFIX: &str = ".requests.jsonl.migrate-";

#[cfg(test)]
use std::cell::Cell;

// Tests set this to a raw errno so `persist` fails after the temp file exists.
#[cfg(test)]
thread_local! {
    static FORCE_PERSIST_ERRNO: Cell<i32> = const { Cell::new(0) };
}

pub fn request_log_path() -> PathBuf {
    request_log_path_for_config_dir(None)
}

pub fn request_log_path_for_args(cli_args: &CliArgs) -> PathBuf {
    request_log_path_for_config_dir(cli_args.config_dir.as_deref())
}

fn request_log_path_for_config_dir(config_dir: Option<&Path>) -> PathBuf {
    env::var_os(REQUEST_LOG_ENV).map_or_else(
        || {
            config_dir
                .map_or_else(CliConfig::get_config_dir, Path::to_path_buf)
                .join(REQUEST_LOG_FILE)
        },
        PathBuf::from,
    )
}

pub fn append_request_record(record: &Value) -> std::io::Result<()> {
    let path = request_log_path();
    append_request_record_at(&path, record)
}

/// Redacts the token in `/invitations/{token}/preview|accept`, matching the
/// route the runner actually sends: it splits off only `?query`, then
/// `Url::set_path` treats `#` as a path character, resolves `.`/`..` (also
/// `%2e`), turns `\` into `/` and drops tabs and newlines. Names match
/// case-insensitively across empty segments. The percent-decoded form also
/// covers servers that decode `%2F` or letters before routing.
pub(crate) fn redact_sensitive_request_path(path: &str) -> String {
    let split_at = path.find('?').unwrap_or(path.len());
    let (route, suffix) = path.split_at(split_at);
    let Ok(mut canonical) = url::Url::parse("http://pcl.invalid/") else {
        return path.to_string();
    };
    canonical.set_path(route);
    let decoded = percent_encoding::percent_decode_str(canonical.path()).decode_utf8_lossy();
    match redact_invitation_tokens(canonical.path()).or_else(|| redact_invitation_tokens(&decoded))
    {
        Some(mut redacted) => {
            redacted.push_str(suffix);
            redacted
        }
        None => path.to_string(),
    }
}

/// Redacts path input echoed back in an error, which may lack the leading `/`.
pub(crate) fn redact_sensitive_path_input(input: &str) -> String {
    if input.starts_with('/') {
        return redact_sensitive_request_path(input);
    }
    let redacted = redact_sensitive_request_path(&format!("/{input}"));
    redacted.strip_prefix('/').unwrap_or(&redacted).to_string()
}

fn redact_invitation_tokens(path: &str) -> Option<String> {
    let mut segments = path.split('/').collect::<Vec<_>>();
    let filled = (0..segments.len())
        .filter(|&index| !segments[index].is_empty())
        .collect::<Vec<_>>();
    let tokens = filled
        .windows(3)
        .filter(|window| {
            segments[window[0]].eq_ignore_ascii_case("invitations")
                && is_invitation_action(segments[window[2]])
        })
        .map(|window| window[1])
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    for index in tokens {
        segments[index] = REDACTED_SEGMENT;
    }
    Some(segments.join("/"))
}

const fn is_invitation_action(segment: &str) -> bool {
    segment.eq_ignore_ascii_case("preview") || segment.eq_ignore_ascii_case("accept")
}

/// Replaces invitation bearer tokens in a proxy error body. `path` is the
/// route that was requested, before history redaction. Status and request id
/// stay on the error; only the echoed token is rewritten.
pub(crate) fn scrub_invitation_tokens_in_value(path: &str, mut body: Value) -> Value {
    let secrets = invitation_secrets_in_path(path);
    if !secrets.is_empty() {
        redact_secrets_in_value(&mut body, &secrets);
    }
    body
}

fn invitation_secrets_in_path(path: &str) -> Vec<String> {
    let split_at = path.find('?').unwrap_or(path.len());
    let route = &path[..split_at];
    let Ok(mut canonical) = url::Url::parse("http://pcl.invalid/") else {
        return Vec::new();
    };
    canonical.set_path(route);
    let decoded = percent_encoding::percent_decode_str(canonical.path()).decode_utf8_lossy();
    let mut secrets = Vec::new();
    collect_invitation_secrets(&mut secrets, route);
    collect_invitation_secrets(&mut secrets, canonical.path());
    collect_invitation_secrets(&mut secrets, &decoded);
    secrets.retain(|secret| !secret.is_empty() && secret != REDACTED_SEGMENT);
    secrets.sort();
    secrets.dedup();
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets
}

fn collect_invitation_secrets(secrets: &mut Vec<String>, path: &str) {
    let segments = path.split('/').collect::<Vec<_>>();
    let filled = (0..segments.len())
        .filter(|&index| !segments[index].is_empty())
        .collect::<Vec<_>>();
    for window in filled.windows(3) {
        if segments[window[0]].eq_ignore_ascii_case("invitations")
            && is_invitation_action(segments[window[2]])
        {
            secrets.push(segments[window[1]].to_string());
        }
    }
}

fn redact_secrets_in_value(value: &mut Value, secrets: &[String]) {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
        Value::String(text) => {
            for secret in secrets {
                if text.contains(secret) {
                    *text = text.replace(secret, REDACTED_SEGMENT);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_secrets_in_value(item, secrets);
            }
        }
        Value::Object(map) => {
            for item in map.values_mut() {
                redact_secrets_in_value(item, secrets);
            }
        }
    }
}

fn redact_record_path(record: &Value) -> Value {
    let mut record = record.clone();
    redact_record_path_in_place(&mut record);
    record
}

fn redact_record_path_in_place(record: &mut Value) {
    let Some(path) = record.get("path").and_then(Value::as_str) else {
        return;
    };
    let redacted = redact_sensitive_request_path(path);
    if redacted == path {
        return;
    }
    let Some(object) = record.as_object_mut() else {
        return;
    };
    object.insert("path".to_string(), Value::String(redacted));
}

/// `/dev/stderr` and `/dev/stdout` are symlinks. They are opened as devices
/// and never created or chmodded. Every other path uses `O_NOFOLLOW`.
#[cfg(unix)]
fn is_supported_log_device(path: &Path) -> bool {
    path == Path::new("/dev/stderr") || path == Path::new("/dev/stdout")
}

fn open_request_log(path: &Path) -> std::io::Result<fs::File> {
    #[cfg(unix)]
    if is_supported_log_device(path) {
        return OpenOptions::new().append(true).open(path);
    }

    let mut options = OpenOptions::new();
    // Read access lets `File::lock` succeed on Windows, which rejects an
    // append-only handle. Writes stay appends.
    options.read(true).create(true).append(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
        // Refuse a symlink. `fchmod` applies to the opened inode, so following
        // a symlink would change and append to some other file.
        options.custom_flags(libc::O_NOFOLLOW);
        let file = options
            .open(path)
            .map_err(|error| refuse_request_log_symlink(path, error))?;
        // A failure fails closed: no record goes to a log we could not restrict.
        if file.metadata()?.is_file() {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        Ok(file)
    }
    #[cfg(not(unix))]
    {
        options.open(path)
    }
}

#[cfg(unix)]
fn refuse_request_log_symlink(path: &Path, error: std::io::Error) -> std::io::Error {
    // `O_NOFOLLOW` reports a symlink as `ELOOP`. Compare the raw code: this
    // nightly still keeps `ErrorKind::FilesystemLoop` unstable.
    if error.raw_os_error() != Some(libc::ELOOP) {
        return error;
    }
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return error;
    };
    if !metadata.file_type().is_symlink() {
        return error;
    }
    std::io::Error::new(
        error.kind(),
        format!(
            "refusing to follow symlink at {}; a request log must be a regular file",
            path.display()
        ),
    )
}

pub fn append_request_record_at(path: &Path, record: &Value) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        // Only missing directories are created, each owner-only from the start.
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(true);
        #[cfg(unix)]
        dirs.mode(0o700);
        dirs.create(parent)?;
    }

    let record = redact_record_path(record);
    let mut bytes = serde_json::to_vec(&record)?;
    bytes.push(b'\n');

    let mut file = open_request_log(path)?;
    // Migration renames a new inode over the path. `lock()` can return a
    // handle for the unlinked file, so reopen until the inode matches.
    if file.metadata()?.is_file() {
        file = lock_opened_request_log(path, file, || open_request_log(path))?;
    }
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn read_request_records(limit: usize) -> std::io::Result<Vec<Value>> {
    let path = request_log_path();
    read_request_records_at(&path, limit)
}

pub fn read_request_records_at(path: &Path, limit: usize) -> std::io::Result<Vec<Value>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    if !request_log_is_rewritable(path)? {
        return read_request_window(path, limit);
    }
    // A clean log stays a shared read: no write handle and no exclusive lock.
    let (records, changed) = scan_readable_request_log(path, limit)?;
    if !changed {
        return Ok(records);
    }
    // A different user, including root, must not replace the owner's inode
    // with a file this process owns.
    if !request_log_owned_by_current_user(path)? {
        return Ok(records);
    }
    match migrate_request_log(path, limit) {
        Ok(migrated) => Ok(migrated),
        Err(error) if log_is_not_writable(&error) => Ok(records),
        Err(error) => Err(error),
    }
}

/// A regular file can be replaced. Symlinks and devices are left untouched:
/// replacing a symlink would either follow it or remove the link.
fn request_log_is_rewritable(path: &Path) -> std::io::Result<bool> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(false);
    }
    #[cfg(unix)]
    if is_supported_log_device(path) {
        return Ok(false);
    }
    Ok(metadata.is_file())
}

fn read_request_window(path: &Path, limit: usize) -> std::io::Result<Vec<Value>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let (records, _) = scan_readable_request_log(path, limit)?;
    Ok(records)
}

fn scan_readable_request_log(path: &Path, limit: usize) -> std::io::Result<(Vec<Value>, bool)> {
    let file = fs::File::open(path)?;
    Ok(scan_request_window(BufReader::new(file), limit))
}

/// Rewrites a legacy log by renaming a new file over `path`. Lines outside
/// the returned window are migrated too.
fn migrate_request_log(path: &Path, limit: usize) -> std::io::Result<Vec<Value>> {
    let file = open_request_log_read_write(path)?;
    let mut file = lock_opened_request_log(path, file, || open_request_log_read_write(path))?;
    let (records, changed) = {
        let reader = BufReader::new(&file);
        scan_request_window(reader, limit)
    };
    if changed && request_log_owned_by_current_user(path)? {
        replace_request_log(path, &mut file)?;
    }
    Ok(records)
}

fn request_log_owned_by_current_user(path: &Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = fs::symlink_metadata(path)?.uid();
        // SAFETY: `geteuid` reads the process credential and has no preconditions.
        let euid = unsafe { libc::geteuid() };
        Ok(uid == euid)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(true)
    }
}

fn open_request_log_read_write(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    options
        .open(path)
        .map_err(|error| map_request_log_open_error(path, error))
}

fn log_is_not_writable(error: &std::io::Error) -> bool {
    if matches!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::ReadOnlyFilesystem
            | std::io::ErrorKind::ResourceBusy
            | std::io::ErrorKind::CrossesDevices
    ) {
        return true;
    }
    #[cfg(unix)]
    return matches!(
        error.raw_os_error(),
        Some(libc::EROFS | libc::EBUSY | libc::EXDEV)
    );
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

/// `file` is already open. After `lock()`, a migration may have renamed a new
/// inode onto `path`, so this loops until the handle is the file at `path`.
fn lock_opened_request_log(
    path: &Path,
    mut file: fs::File,
    mut reopen: impl FnMut() -> std::io::Result<fs::File>,
) -> std::io::Result<fs::File> {
    if !file.metadata()?.is_file() {
        return Ok(file);
    }
    loop {
        file.lock()?;
        if log_handle_matches_path(path, &file)? {
            return Ok(file);
        }
        file = reopen()?;
        if !file.metadata()?.is_file() {
            return Ok(file);
        }
    }
}

fn log_handle_matches_path(path: &Path, file: &fs::File) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        let current = fs::symlink_metadata(path)?;
        Ok(opened.ino() == current.ino() && opened.dev() == current.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, file);
        Ok(true)
    }
}

fn map_request_log_open_error(path: &Path, error: std::io::Error) -> std::io::Error {
    #[cfg(unix)]
    {
        refuse_request_log_symlink(path, error)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        error
    }
}

fn scan_request_window<R: BufRead>(reader: R, limit: usize) -> (Vec<Value>, bool) {
    let mut records = VecDeque::new();
    let mut changed = false;
    for line in reader.lines() {
        let Ok(line) = line else {
            // A partial scan must not replace the rest of the file.
            return (Vec::from(records), false);
        };
        changed |= note_request_line(&mut records, &line, limit);
    }
    (Vec::from(records), changed)
}

fn note_request_line(records: &mut VecDeque<Value>, line: &str, limit: usize) -> bool {
    let Ok(mut record) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let changed = request_path_needs_redaction(&record);
    if limit == 0 {
        return changed;
    }
    redact_record_path_in_place(&mut record);
    if records.len() == limit {
        records.pop_front();
    }
    records.push_back(record);
    changed
}

fn request_path_needs_redaction(record: &Value) -> bool {
    record
        .get("path")
        .and_then(Value::as_str)
        .is_some_and(|path| redact_sensitive_request_path(path) != path)
}

fn replace_request_log(path: &Path, file: &mut fs::File) -> std::io::Result<()> {
    let parent = request_log_parent(path);
    remove_stale_migration_files(parent)?;
    file.seek(SeekFrom::Start(0))?;
    let mut rewritten = named_migration_file(parent)?;
    copy_migrated_lines(file, &mut rewritten)?;
    rewritten.as_file().sync_all()?;
    // `persist` renames over `path`. The old inode stays intact until the
    // rename, so a crash cannot leave a 0-byte log. The caller still holds
    // the old inode's lock and drops it after this returns.
    persist_over_log(rewritten, path)
}

fn persist_over_log(rewritten: tempfile::NamedTempFile, path: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    {
        let forced = FORCE_PERSIST_ERRNO.with(Cell::get);
        if forced != 0 {
            let _ = rewritten.close();
            return Err(std::io::Error::from_raw_os_error(forced));
        }
    }
    let parent = request_log_parent(path);
    match rewritten.persist(path) {
        Ok(_) => fsync_dir(parent),
        Err(error) => {
            let io_error = error.error;
            // Dropping the `NamedTempFile` unlinks it. `close` does that
            // before this function returns, including for `EBUSY` and `EXDEV`.
            let _ = error.file.close();
            Err(io_error)
        }
    }
}

fn remove_stale_migration_files(parent: &Path) -> std::io::Result<()> {
    let entries = fs::read_dir(parent)?;
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if !name.starts_with(MIGRATION_TEMP_PREFIX) {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                ) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn named_migration_file(parent: &Path) -> std::io::Result<tempfile::NamedTempFile> {
    let mut builder = tempfile::Builder::new();
    builder.prefix(MIGRATION_TEMP_PREFIX);
    #[cfg(unix)]
    builder.permissions(fs::Permissions::from_mode(0o600));
    builder.tempfile_in(parent)
}

fn request_log_parent(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn fsync_dir(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

fn copy_migrated_lines(file: &mut fs::File, output: &mut impl Write) -> std::io::Result<()> {
    let reader = BufReader::new(std::io::Read::by_ref(file));
    for stored in reader.lines() {
        let stored = stored?;
        write_migrated_line(&stored, output)?;
    }
    Ok(())
}

fn write_migrated_line(line: &str, output: &mut impl Write) -> std::io::Result<()> {
    if let Some(redacted) = redacted_request_line(line)? {
        output.write_all(redacted.as_bytes())?;
    } else {
        output.write_all(line.as_bytes())?;
    }
    output.write_all(b"\n")?;
    Ok(())
}

fn redacted_request_line(line: &str) -> std::io::Result<Option<String>> {
    let Ok(mut record) = serde_json::from_str::<Value>(line) else {
        return Ok(None);
    };
    if !request_path_needs_redaction(&record) {
        return Ok(None);
    }
    redact_record_path_in_place(&mut record);
    serde_json::to_string(&record)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

pub fn clear_request_log() -> std::io::Result<bool> {
    let path = request_log_path();
    clear_request_log_at(&path)
}

pub fn clear_request_log_at(path: &Path) -> std::io::Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{
        Value,
        json,
    };
    use std::{
        alloc::{
            GlobalAlloc,
            Layout,
            System,
        },
        cell::Cell,
        io::Write,
    };

    /// Counts outstanding bytes on this thread while a read is in progress.
    /// A second full-log buffer shows up as a peak at least as large as the file.
    struct TrackingAllocator;

    #[global_allocator]
    static TRACKING_ALLOCATOR: TrackingAllocator = TrackingAllocator;

    thread_local! {
        static ALLOC_CURRENT: Cell<usize> = const { Cell::new(0) };
        static ALLOC_PEAK: Cell<usize> = const { Cell::new(0) };
        static ALLOC_TRACKING: Cell<bool> = const { Cell::new(false) };
        static ALLOC_IN_HOOK: Cell<bool> = const { Cell::new(false) };
    }

    unsafe impl GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            // SAFETY: `layout` is the layout the allocator caller asked for.
            let ptr = unsafe { System.alloc(layout) };
            if !ptr.is_null() {
                note_alloc(layout.size(), true);
            }
            ptr
        }

        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            note_alloc(layout.size(), false);
            // SAFETY: `ptr` was allocated by `System` with `layout`.
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    fn note_alloc(size: usize, allocate: bool) {
        let _ = ALLOC_IN_HOOK.try_with(|hook| {
            if hook.get() {
                return;
            }
            hook.set(true);
            let _ = ALLOC_TRACKING.try_with(|tracking| {
                if !tracking.get() {
                    return;
                }
                let _ = ALLOC_CURRENT.try_with(|current| {
                    let next = if allocate {
                        current.get().saturating_add(size)
                    } else {
                        current.get().saturating_sub(size)
                    };
                    current.set(next);
                    let _ = ALLOC_PEAK.try_with(|peak| {
                        if next > peak.get() {
                            peak.set(next);
                        }
                    });
                });
            });
            hook.set(false);
        });
    }

    fn peak_allocated_bytes(body: impl FnOnce()) -> usize {
        ALLOC_CURRENT.with(|current| current.set(0));
        ALLOC_PEAK.with(|peak| peak.set(0));
        ALLOC_TRACKING.with(|tracking| tracking.set(true));
        body();
        ALLOC_TRACKING.with(|tracking| tracking.set(false));
        ALLOC_PEAK.with(Cell::get)
    }

    #[test]
    fn append_request_record_at_writes_valid_jsonl_under_concurrency() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let writers = 16;
        let records_per_writer = 25;

        let handles = (0..writers)
            .map(|writer| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for sequence in 0..records_per_writer {
                        append_request_record_at(
                            &path,
                            &json!({
                                "timestamp": "2026-05-07T00:00:00Z",
                                "kind": "workflow",
                                "method": "POST",
                                "path": format!("/projects/{writer}/{sequence}"),
                                "status": 200,
                                "success": true,
                                "request_id": format!("req_{writer}_{sequence}"),
                                "padding": "x".repeat(4096),
                            }),
                        )
                        .expect("append request record");
                    }
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().expect("writer thread should not panic");
        }

        let content = fs::read_to_string(&path).expect("read request log");
        let lines = content.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), writers * records_per_writer);
        for line in lines {
            serde_json::from_str::<Value>(line).expect("request log line should be valid json");
        }
    }

    const INVITATION_TOKEN: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn invitation_record(method: &str, route: &str, operation_id: &str) -> Value {
        json!({
            "timestamp": "2026-11-02T00:00:00Z",
            "kind": "workflow",
            "method": method,
            "path": route,
            "status": 200,
            "success": true,
            "request_id": "poc-response",
            "operation_id": operation_id,
        })
    }

    #[test]
    fn invitation_tokens_are_redacted_in_new_request_history() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("state").join("requests.jsonl");
        let preview = format!("/invitations/{INVITATION_TOKEN}/preview");
        let accept = format!("/invitations/{INVITATION_TOKEN}/accept");

        append_request_record_at(
            &path,
            &invitation_record("GET", &preview, "previewInvitation"),
        )
        .expect("append preview");
        append_request_record_at(
            &path,
            &invitation_record("POST", &accept, "acceptInvitation"),
        )
        .expect("append accept");
        append_request_record_at(
            &path,
            &invitation_record(
                "DELETE",
                "/projects/project-1/invitations/invite-1",
                "revokeInvitation",
            ),
        )
        .expect("append revoke");

        let content = fs::read_to_string(&path).expect("read request log");
        assert!(
            !content.contains(INVITATION_TOKEN),
            "request history persisted the invitation token: {content}"
        );
        assert!(content.contains("/invitations/<redacted>/preview"));
        assert!(content.contains("/invitations/<redacted>/accept"));
        assert!(content.contains("/projects/project-1/invitations/invite-1"));

        let records = read_request_records_at(&path, 10).expect("read records");
        let paths = records
            .iter()
            .map(|record| record["path"].as_str().unwrap_or_default())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                "/invitations/<redacted>/preview",
                "/invitations/<redacted>/accept",
                "/projects/project-1/invitations/invite-1",
            ]
        );
    }

    #[test]
    fn existing_request_history_does_not_return_invitation_tokens() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let route = format!("/invitations/{INVITATION_TOKEN}/preview?unused=1");
        fs::write(
            &path,
            format!(
                "{}\n",
                invitation_record("GET", &route, "previewInvitation")
            ),
        )
        .expect("seed request log");

        let records = read_request_records_at(&path, 10).expect("read records");
        let rendered = records[0].to_string();
        assert!(
            !rendered.contains(INVITATION_TOKEN),
            "pcl requests list returned the invitation token: {rendered}"
        );
        assert_eq!(
            records[0]["path"],
            "/invitations/<redacted>/preview?unused=1"
        );
    }

    #[test]
    fn read_request_records_at_keeps_only_the_last_limit_then_redacts() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let dropped = format!("/invitations/{INVITATION_TOKEN}/preview");
        let kept = format!("/invitations/{INVITATION_TOKEN}/accept");
        let mut lines = Vec::new();
        for request_id in ["one", "two"] {
            lines.push(json!({"path": dropped, "request_id": request_id}).to_string());
        }
        lines.push("not json".to_string());
        lines.push(json!({"path": "/health/3", "request_id": "three"}).to_string());
        lines.push(json!({"path": kept, "request_id": "four"}).to_string());
        let mut body = lines.join("\n");
        body.push('\n');
        fs::write(&path, body).expect("seed request log");

        let records = read_request_records_at(&path, 2).expect("read records");
        let rendered = format!("{records:?}");
        assert!(
            !rendered.contains(INVITATION_TOKEN),
            "the kept window still contains the invitation token: {rendered}"
        );
        assert_eq!(
            records
                .iter()
                .map(|record| record["request_id"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["three", "four"]
        );
        assert_eq!(records[0]["path"], "/health/3");
        assert_eq!(records[1]["path"], "/invitations/<redacted>/accept");
        assert!(
            read_request_records_at(&path, 0)
                .expect("read none")
                .is_empty()
        );
    }

    #[test]
    fn read_request_records_at_removes_legacy_invitation_tokens_from_disk() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let dropped = format!("/invitations/{INVITATION_TOKEN}/preview");
        let kept = format!("/invitations/{INVITATION_TOKEN}/accept");
        let health = json!({"path": "/health/3", "request_id": "three"}).to_string();
        let lines = [
            json!({"path": dropped, "request_id": "dropped"}).to_string(),
            "not json".to_string(),
            health.clone(),
            json!({"path": kept, "request_id": "four"}).to_string(),
        ];
        let mut body = lines.join("\n");
        body.push('\n');
        fs::write(&path, &body).expect("seed request log");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loosen log mode");
        }

        let records = read_request_records_at(&path, 1).expect("migrate on read");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0]["request_id"], "four");
        assert_eq!(records[0]["path"], "/invitations/<redacted>/accept");
        assert!(
            !records[0].to_string().contains(INVITATION_TOKEN),
            "returned record kept the invitation token: {}",
            records[0]
        );

        let migrated = fs::read_to_string(&path).expect("read migrated log");
        assert!(
            !migrated.contains(INVITATION_TOKEN),
            "legacy invitation token stayed on disk: {migrated}"
        );
        assert!(migrated.contains("\"request_id\":\"dropped\""));
        assert!(migrated.contains("/invitations/<redacted>/preview"));
        assert!(migrated.contains("/invitations/<redacted>/accept"));
        assert!(migrated.contains("not json\n"));
        assert!(migrated.contains(&health));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let migrated_bytes = fs::read(&path).expect("snapshot migrated log");
        read_request_records_at(&path, 10).expect("second read");
        assert_eq!(
            fs::read(&path).expect("stable reread"),
            migrated_bytes,
            "a second read rewrote a migrated log"
        );

        append_request_record_at(&path, &json!({"path": "/health", "request_id": "later"}))
            .expect("append after migration");
        let after_append = fs::read_to_string(&path).expect("read log after append");
        assert!(
            !after_append.contains(INVITATION_TOKEN),
            "append restored the invitation token: {after_append}"
        );
        assert!(after_append.contains("\"request_id\":\"later\""));
    }

    #[test]
    fn read_request_records_at_migrates_even_when_the_window_is_empty() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let route = format!("/invitations/{INVITATION_TOKEN}/preview");
        fs::write(&path, format!("{}\n", json!({"path": route}))).expect("seed request log");

        let records = read_request_records_at(&path, 0).expect("read none");
        assert!(records.is_empty());
        let migrated = fs::read_to_string(&path).expect("read migrated log");
        assert!(
            !migrated.contains(INVITATION_TOKEN),
            "limit 0 left the invitation token on disk: {migrated}"
        );
        assert!(migrated.contains("/invitations/<redacted>/preview"));
    }

    #[test]
    fn read_request_records_at_leaves_a_clean_log_on_disk() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let body = "{\"path\":\"/health\"}\nnot json\n";
        fs::write(&path, body).expect("seed request log");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loosen log mode");
        }

        let records = read_request_records_at(&path, 10).expect("read records");
        assert_eq!(records[0]["path"], "/health");
        assert_eq!(fs::read(&path).expect("log bytes"), body.as_bytes());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o644);
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_only_log_is_listed_and_left_unchanged() {
        use std::os::unix::fs::PermissionsExt;
        let temp_dir = tempfile::tempdir().expect("create temp dir");

        let clean = temp_dir.path().join("clean.jsonl");
        let clean_body = "{\"path\":\"/health\"}\n";
        fs::write(&clean, clean_body).expect("seed clean log");
        fs::set_permissions(&clean, fs::Permissions::from_mode(0o400)).expect("chmod clean log");
        let records = read_request_records_at(&clean, 10).expect("read 0400 clean log");
        assert_eq!(records[0]["path"], "/health");
        assert_eq!(
            fs::read(&clean).expect("clean bytes"),
            clean_body.as_bytes()
        );
        assert_eq!(
            fs::metadata(&clean)
                .expect("clean metadata")
                .permissions()
                .mode()
                & 0o777,
            0o400
        );

        let legacy = temp_dir.path().join("legacy.jsonl");
        let route = format!("/invitations/{INVITATION_TOKEN}/preview");
        let legacy_body = format!("{}\n", json!({"path": route, "request_id": "legacy"}));
        fs::write(&legacy, &legacy_body).expect("seed legacy log");
        fs::set_permissions(&legacy, fs::Permissions::from_mode(0o400)).expect("chmod legacy log");
        let records = read_request_records_at(&legacy, 10).expect("read 0400 legacy log");
        assert_eq!(records[0]["path"], "/invitations/<redacted>/preview");
        assert!(
            !records[0].to_string().contains(INVITATION_TOKEN),
            "0400 list returned the invitation token: {}",
            records[0]
        );
        assert_eq!(
            fs::read(&legacy).expect("legacy bytes"),
            legacy_body.as_bytes()
        );
        assert_eq!(
            fs::metadata(&legacy)
                .expect("legacy metadata")
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
    }

    #[cfg(unix)]
    #[test]
    fn migration_renames_a_new_file_over_the_legacy_log() {
        use std::os::unix::fs::{
            MetadataExt,
            PermissionsExt,
        };
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let route = format!("/invitations/{INVITATION_TOKEN}/preview");
        fs::write(
            &path,
            format!("{}\n", json!({"path": route, "request_id": "legacy"})),
        )
        .expect("seed request log");
        let seeded_inode = fs::metadata(&path).expect("seed metadata").ino();

        let records = read_request_records_at(&path, 10).expect("migrate on read");
        assert_eq!(records[0]["path"], "/invitations/<redacted>/preview");
        let migrated_inode = fs::metadata(&path).expect("migrated metadata").ino();
        assert_ne!(
            migrated_inode, seeded_inode,
            "migration rewrote the log in place"
        );
        let migrated = fs::read_to_string(&path).expect("read migrated log");
        assert!(
            !migrated.contains(INVITATION_TOKEN),
            "renamed log kept the invitation token: {migrated}"
        );
        assert!(migrated.contains("/invitations/<redacted>/preview"));
        assert!(migrated.contains("\"request_id\":\"legacy\""));
        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);

        append_request_record_at(&path, &json!({"path": "/health", "request_id": "after"}))
            .expect("append after rename");
        assert_eq!(
            fs::metadata(&path).expect("append metadata").ino(),
            migrated_inode,
            "append wrote a different file than the renamed log"
        );
        let after = fs::read_to_string(&path).expect("read log after append");
        assert!(after.contains("\"request_id\":\"legacy\""));
        assert!(after.contains("\"request_id\":\"after\""));
        assert!(!after.contains(INVITATION_TOKEN));
    }

    #[cfg(unix)]
    #[test]
    fn append_reopens_when_the_locked_log_was_replaced() {
        use std::os::unix::fs::MetadataExt;
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        fs::write(&path, "old\n").expect("seed log");
        let stale = open_request_log(&path).expect("open log");
        let stale_inode = stale.metadata().expect("stale metadata").ino();
        let replacement = temp_dir.path().join("replacement.jsonl");
        fs::write(&replacement, "new\n").expect("write replacement");
        fs::rename(&replacement, &path).expect("replace log");
        assert_ne!(
            fs::metadata(&path).expect("new metadata").ino(),
            stale_inode
        );

        let mut current = lock_opened_request_log(&path, stale, || open_request_log(&path))
            .expect("reopen replaced log");
        assert_eq!(
            current.metadata().expect("current metadata").ino(),
            fs::metadata(&path).expect("path metadata").ino()
        );
        current.write_all(b"appended\n").expect("append");
        current.sync_all().expect("sync");
        drop(current);
        assert_eq!(
            fs::read_to_string(&path).expect("read log"),
            "new\nappended\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn busy_or_cross_device_replace_keeps_the_log_and_removes_the_temp_file() {
        use std::os::unix::fs::MetadataExt;
        assert!(log_is_not_writable(&std::io::Error::from_raw_os_error(
            libc::EBUSY
        )));
        assert!(log_is_not_writable(&std::io::Error::from_raw_os_error(
            libc::EXDEV
        )));
        assert!(log_is_not_writable(&std::io::Error::new(
            std::io::ErrorKind::ResourceBusy,
            "busy"
        )));
        assert!(log_is_not_writable(&std::io::Error::new(
            std::io::ErrorKind::CrossesDevices,
            "cross"
        )));

        for errno in [libc::EBUSY, libc::EXDEV] {
            let temp_dir = tempfile::tempdir().expect("create temp dir");
            let path = temp_dir.path().join("requests.jsonl");
            let route = format!("/invitations/{INVITATION_TOKEN}/preview");
            let body = format!("{}\n", json!({"path": route, "request_id": "legacy"}));
            fs::write(&path, &body).expect("seed request log");
            let stale = temp_dir
                .path()
                .join(format!("{MIGRATION_TEMP_PREFIX}stale"));
            fs::write(&stale, b"partial").expect("seed stale temp");

            FORCE_PERSIST_ERRNO.with(|errno_cell| errno_cell.set(errno));
            let _reset_persist_errno = ResetPersistErrno;
            let records = read_request_records_at(&path, 10)
                .expect("list when rename cannot replace the log");

            assert_eq!(records[0]["path"], "/invitations/<redacted>/preview");
            assert!(!records[0].to_string().contains(INVITATION_TOKEN));
            assert_eq!(fs::read(&path).expect("log bytes"), body.as_bytes());
            assert!(
                !stale.exists(),
                "stale migration temp survived a failed replace"
            );
            assert!(
                migration_temp_names(temp_dir.path()).is_empty(),
                "failed replace left a migration temp file"
            );
        }

        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let body = "{\"path\":\"/health\"}\n";
        fs::write(&path, body).expect("seed destination");
        let shm = Path::new("/dev/shm");
        if fs::metadata(shm).expect("shm metadata").dev()
            != fs::metadata(temp_dir.path()).expect("dir metadata").dev()
        {
            let mut builder = tempfile::Builder::new();
            builder.prefix(MIGRATION_TEMP_PREFIX);
            let rewritten = builder
                .tempfile_in(shm)
                .expect("temp on another filesystem");
            let temp_path = rewritten.path().to_path_buf();
            let error = persist_over_log(rewritten, &path).expect_err("cross-device rename");
            assert!(
                log_is_not_writable(&error),
                "EXDEV was not treated as a kept log: {error}"
            );
            assert_eq!(fs::read(&path).expect("destination bytes"), body.as_bytes());
            assert!(
                !temp_path.exists(),
                "cross-device temp file was left behind"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn migration_skips_a_log_owned_by_another_user() {
        use std::os::unix::fs::{
            MetadataExt,
            PermissionsExt,
        };
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let route = format!("/invitations/{INVITATION_TOKEN}/preview");
        let body = format!("{}\n", json!({"path": route, "request_id": "legacy"}));
        fs::write(&path, &body).expect("seed request log");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).expect("widen log");
        disown_log(&path);

        let owner = fs::metadata(&path).expect("metadata").uid();
        // SAFETY: `geteuid` reads the process credential and has no preconditions.
        let euid = unsafe { libc::geteuid() };
        assert_ne!(owner, euid);
        let records = read_request_records_at(&path, 10).expect("list another user's log");
        assert_eq!(records[0]["path"], "/invitations/<redacted>/preview");
        assert!(!records[0].to_string().contains(INVITATION_TOKEN));
        assert_eq!(fs::read(&path).expect("log bytes"), body.as_bytes());
        let metadata = fs::metadata(&path).expect("metadata after list");
        assert_eq!(metadata.uid(), owner);
        assert_eq!(metadata.permissions().mode() & 0o777, 0o666);
        assert!(
            migration_temp_names(temp_dir.path()).is_empty(),
            "skipped migration created a temp file"
        );
    }

    #[test]
    fn migration_removes_stale_temp_files_before_replacing_the_log() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let route = format!("/invitations/{INVITATION_TOKEN}/preview");
        fs::write(
            &path,
            format!("{}\n", json!({"path": route, "request_id": "legacy"})),
        )
        .expect("seed request log");
        let stale = temp_dir
            .path()
            .join(format!("{MIGRATION_TEMP_PREFIX}interrupted"));
        fs::write(&stale, b"redacted partial").expect("seed stale temp");
        let unrelated = temp_dir.path().join(".tmpkeep");
        fs::write(&unrelated, b"keep").expect("seed unrelated file");

        let records = read_request_records_at(&path, 10).expect("migrate on read");
        assert_eq!(records[0]["path"], "/invitations/<redacted>/preview");
        assert!(!stale.exists(), "stale migration temp was left in place");
        assert_eq!(fs::read(&unrelated).expect("unrelated file"), b"keep");
        let migrated = fs::read_to_string(&path).expect("read migrated log");
        assert!(!migrated.contains(INVITATION_TOKEN));
        assert!(migrated.contains("/invitations/<redacted>/preview"));
        assert!(
            migration_temp_names(temp_dir.path()).is_empty(),
            "migration left its temp file behind"
        );
    }

    fn migration_temp_names(parent: &Path) -> Vec<String> {
        fs::read_dir(parent)
            .expect("read log dir")
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.starts_with(MIGRATION_TEMP_PREFIX))
            .collect()
    }

    struct ResetPersistErrno;

    impl Drop for ResetPersistErrno {
        fn drop(&mut self) {
            FORCE_PERSIST_ERRNO.with(|errno_cell| errno_cell.set(0));
        }
    }

    #[cfg(unix)]
    fn disown_log(path: &Path) {
        // SAFETY: `geteuid` reads the process credential and has no preconditions.
        let as_root = unsafe { libc::geteuid() } == 0;
        let mut command = if as_root {
            std::process::Command::new("chown")
        } else {
            let mut command = std::process::Command::new("sudo");
            command.args(["-n", "chown"]);
            command
        };
        let status = command
            .args(["nobody:nogroup", "--"])
            .arg(path)
            .status()
            .expect("chown log to nobody");
        assert!(status.success(), "could not chown the log to nobody");
    }

    #[test]
    fn limited_read_does_not_retain_the_whole_log_and_still_migrates() {
        const LINES: usize = 3_000;
        const PADDING: usize = 512;
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let clean = temp_dir.path().join("clean.jsonl");
        let mut clean_file = fs::File::create(&clean).expect("create clean log");
        for index in 0..LINES {
            let padding = "x".repeat(PADDING);
            writeln!(
                clean_file,
                r#"{{"path":"/health/{index}","padding":"{padding}"}}"#
            )
            .expect("write clean line");
        }
        clean_file.sync_all().expect("sync clean log");
        drop(clean_file);
        let clean_bytes = fs::read(&clean).expect("snapshot clean log");
        let clean_len = clean_bytes.len();
        assert!(
            clean_len > 1_000_000,
            "fixture is too small to prove a bound"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&clean, fs::Permissions::from_mode(0o644)).expect("loosen mode");
        }

        let peak = peak_allocated_bytes(|| {
            let records = read_request_records_at(&clean, 10).expect("read clean log");
            assert_eq!(records.len(), 10);
            assert_eq!(records[9]["path"], format!("/health/{}", LINES - 1));
        });
        assert!(
            peak < clean_len / 2,
            "clean limited read retained {peak} bytes of a {clean_len} byte log"
        );
        assert_eq!(fs::read(&clean).expect("reread clean log"), clean_bytes);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&clean).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o644);
        }

        let legacy = temp_dir.path().join("legacy.jsonl");
        let mut legacy_file = fs::File::create(&legacy).expect("create legacy log");
        let token_line = json!({
            "path": format!("/invitations/{INVITATION_TOKEN}/preview"),
            "request_id": "legacy-0",
        })
        .to_string();
        writeln!(legacy_file, "{token_line}").expect("write legacy token");
        for index in 1..LINES {
            let padding = "x".repeat(PADDING);
            writeln!(
                legacy_file,
                r#"{{"path":"/health/{index}","padding":"{padding}"}}"#
            )
            .expect("write legacy line");
        }
        legacy_file.sync_all().expect("sync legacy log");
        drop(legacy_file);
        let legacy_len = usize::try_from(fs::metadata(&legacy).expect("legacy metadata").len())
            .expect("legacy log length fits in usize");

        let peak = peak_allocated_bytes(|| {
            let records = read_request_records_at(&legacy, 10).expect("migrate legacy log");
            assert_eq!(records.len(), 10);
            assert!(
                !records
                    .iter()
                    .any(|record| record.to_string().contains(INVITATION_TOKEN))
            );
        });
        assert!(
            peak < legacy_len / 2,
            "legacy migration retained {peak} bytes of a {legacy_len} byte log"
        );
        let migrated = fs::read_to_string(&legacy).expect("read migrated log");
        assert!(
            !migrated.contains(INVITATION_TOKEN),
            "legacy token stayed on disk"
        );
        assert!(migrated.contains("\"request_id\":\"legacy-0\""));
        assert!(migrated.contains("/invitations/<redacted>/preview"));
        assert!(migrated.contains(&format!("/health/{}", LINES - 1)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&legacy)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn read_request_records_at_does_not_follow_a_symlink_log() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let target = temp_dir.path().join("target.jsonl");
        let link = temp_dir.path().join("link.jsonl");
        let route = format!("/invitations/{INVITATION_TOKEN}/preview");
        let seeded = format!("{}\n", json!({"path": route}));
        fs::write(&target, &seeded).expect("seed target");
        std::os::unix::fs::symlink(&target, &link).expect("symlink log");

        let records = read_request_records_at(&link, 10).expect("read through symlink");
        assert_eq!(records[0]["path"], "/invitations/<redacted>/preview");
        assert!(!records[0].to_string().contains(INVITATION_TOKEN));
        assert_eq!(fs::read_to_string(&target).expect("target bytes"), seeded);
        assert!(
            fs::symlink_metadata(&link)
                .expect("link metadata")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn scrub_invitation_tokens_in_value_rewrites_echoed_urls() {
        let path = format!("/invitations/{INVITATION_TOKEN}/preview?x=1");
        let body = scrub_invitation_tokens_in_value(
            &path,
            json!({
                "error": "invitation not found",
                "url": format!("https://api.example/api/v1/invitations/{INVITATION_TOKEN}/preview"),
                "nested": [{"note": format!("token {INVITATION_TOKEN}")}],
            }),
        );
        let rendered = body.to_string();
        assert!(
            !rendered.contains(INVITATION_TOKEN),
            "echoed invitation token stayed in the body: {rendered}"
        );
        assert_eq!(body["error"], "invitation not found");
        assert!(
            body["url"]
                .as_str()
                .unwrap_or_default()
                .contains("/invitations/<redacted>/preview")
        );
        assert!(
            body["nested"][0]["note"]
                .as_str()
                .unwrap_or_default()
                .contains("<redacted>")
        );

        let untouched = scrub_invitation_tokens_in_value(
            "/health",
            json!({"url": format!("/invitations/{INVITATION_TOKEN}/preview")}),
        );
        assert!(untouched.to_string().contains(INVITATION_TOKEN));
    }

    /// Inputs that `Url::set_path` sends to the live preview/accept route.
    fn invitation_route_variants() -> Vec<String> {
        let t = INVITATION_TOKEN;
        [
            "/invitations/{t}/./preview",
            "/invitations/{t}/%2e/preview",
            "/invitations/{t}/%2E/accept",
            "/invitations/{t}/junk/../preview",
            "/invitations/{t}/junk/%2e%2e/accept",
            "/invitations/{t}/junk/%2E%2e/preview",
            "/invitations/{t}/junk/.%2e/accept",
            "/invitations/{t}\\preview",
            "\\invitations\\{t}\\accept",
            "/invitations/{t}/pre\tview",
            "/Invitations/{t}/Preview",
            "/INVITATIONS/{t}/ACCEPT",
            "/%69nvitations/{t}/%70review",
            "/invitations//{t}/preview",
            "/invitations/{t}//accept",
            "/invitations/{t}/preview/",
            "/invitations/{t}/accept?x=1#frag",
            "/api/v1/invitations/{t}/preview",
            "/x/../invitations/{t}/accept",
            "/invitations/{t}/<redacted>/../preview",
            "/invitations/{t}%2Fpreview",
            "/invitations/{t}%2faccept",
            "/invitations/{t}/pre%76iew",
            "/invitations/{t}%2F..%2Fx/preview",
            "/x#/../invitations/{t}/preview",
            "/x#y#/../invitations/{t}/accept",
            "/x%23/../invitations/{t}/preview",
            "/x%3F/../invitations/{t}/accept",
            "/x%3f/..%2F../invitations/{t}/preview",
        ]
        .iter()
        .map(|template| template.replace("{t}", t))
        .collect()
    }

    #[test]
    fn equivalent_invitation_route_forms_are_redacted_on_write() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        for route in invitation_route_variants() {
            append_request_record_at(
                &path,
                &invitation_record("GET", &route, "previewInvitation"),
            )
            .expect("append variant");
        }
        let content = fs::read_to_string(&path).expect("read request log");
        assert!(
            !content.contains(INVITATION_TOKEN),
            "request history persisted the invitation token: {content}"
        );
    }

    #[test]
    fn equivalent_invitation_route_forms_in_existing_history_are_redacted_on_read() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let path = temp_dir.path().join("requests.jsonl");
        let seeded = invitation_route_variants()
            .iter()
            .map(|route| invitation_record("GET", route, "previewInvitation").to_string() + "\n")
            .collect::<String>();
        fs::write(&path, seeded).expect("seed request log");

        let records = read_request_records_at(&path, 100).expect("read records");
        assert_eq!(records.len(), invitation_route_variants().len());
        for record in records {
            let rendered = record.to_string();
            assert!(
                !rendered.contains(INVITATION_TOKEN),
                "pcl requests list returned the invitation token: {rendered}"
            );
            assert!(rendered.contains("<redacted>"), "{rendered}");
        }
    }

    #[test]
    fn neighbouring_routes_and_input_markers_are_left_unchanged() {
        for route in [
            "/projects/project-1/invitations/invite-1",
            "/projects/project-1/invitations/invite-1/resend",
            "/invitations/pending",
            "/invitations/token/other",
            "/invitations/token/previews",
            "/invitationsx/token/accept",
            "/invitations-archive/item/preview",
            "/invitations/item/preview-backup",
            "/invitations/item/accepted",
            "/my-invitations/T/preview",
            "/invitations/item/preview;x",
            "/invitations/item/preview%23x",
            "/invitations;v=1/item/accept.json",
            "/invitations/item/preview#frag",
            "/health?next=/invitations/x/preview",
            "/notes/<redacted>/preview",
            "/projects/a/../b",
            "relative/invitations",
            "",
        ] {
            assert_eq!(redact_sensitive_request_path(route), route);
        }
        assert_eq!(
            redact_sensitive_request_path("/invitations/<redacted>/./preview"),
            "/invitations/<redacted>/preview"
        );
    }

    #[test]
    fn redaction_stays_linear_on_hostile_history_lines() {
        const SIZE: usize = 64 * 1024;
        let inputs = [
            "/invitations/".repeat(SIZE / "/invitations/".len()),
            format!("/{}", "a".repeat(SIZE)),
            "/./../%2e/%2e%2e/".repeat(SIZE / "/./../%2e/%2e%2e/".len()),
            "/invitations/t/preview".repeat(SIZE / "/invitations/t/preview".len()),
        ];
        for input in &inputs {
            let started = std::time::Instant::now();
            let _ = redact_sensitive_request_path(input);
            let elapsed = started.elapsed();
            eprintln!("redact {} bytes: {elapsed:?}", input.len());
            assert!(
                elapsed < std::time::Duration::from_secs(1),
                "redaction took {elapsed:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn every_directory_created_for_a_new_log_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let outer = temp_dir.path().join("a");
        let inner = outer.join("b");
        append_request_record_at(
            &inner.join("requests.jsonl"),
            &json!({"path": "/health", "status": 200}),
        )
        .expect("append nested log");
        for dir in [&outer, &inner] {
            let mode = fs::metadata(dir)
                .expect("dir metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700, "{} mode was {mode:o}", dir.display());
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_regular_log_targets_are_not_chmodded() {
        use std::os::unix::fs::PermissionsExt;

        let dev_null = Path::new("/dev/null");
        let before = fs::metadata(dev_null)
            .expect("dev null")
            .permissions()
            .mode();
        open_request_log(dev_null).expect("open a character device without chmod");
        let after = fs::metadata(dev_null)
            .expect("dev null")
            .permissions()
            .mode();
        assert_eq!(before, after);
    }

    #[cfg(unix)]
    #[test]
    fn stderr_and_stdout_are_not_refused_as_symlinks() {
        use std::os::unix::fs::PermissionsExt;

        for device in ["/dev/stderr", "/dev/stdout"] {
            let path = Path::new(device);
            let before = fs::metadata(path)
                .expect("device metadata")
                .permissions()
                .mode();
            // These paths are symlinks. `O_NOFOLLOW` would fail with `ELOOP`.
            // Reopening stderr can fail in a sandbox; that is not a symlink refusal.
            match open_request_log(path) {
                Ok(_) => {
                    let after = fs::metadata(path)
                        .expect("device metadata")
                        .permissions()
                        .mode();
                    assert_eq!(before, after, "{device} mode changed");
                }
                Err(error) => {
                    assert_ne!(
                        error.raw_os_error(),
                        Some(libc::ELOOP),
                        "{device} was refused as a symlink: {error}"
                    );
                    assert!(
                        !error.to_string().contains("refusing to follow symlink"),
                        "{device} was refused as a symlink: {error}"
                    );
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn request_log_refuses_to_follow_a_symlink() {
        use std::os::unix::fs::{
            PermissionsExt,
            symlink,
        };

        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let target = temp_dir.path().join("unrelated.jsonl");
        fs::write(&target, "keep\n").expect("seed target");
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).expect("widen target");
        let link = temp_dir.path().join("requests.jsonl");
        symlink(&target, &link).expect("symlink log");

        let error = append_request_record_at(&link, &json!({"path": "/health", "status": 200}))
            .expect_err("symlink log should be refused");
        let message = error.to_string();
        assert!(message.contains("refusing to follow symlink"), "{message}");
        assert_eq!(fs::read_to_string(&target).expect("read target"), "keep\n");
        let mode = fs::metadata(&target)
            .expect("target metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o644, "symlink target mode changed to {mode:o}");
        assert!(
            fs::symlink_metadata(&link)
                .expect("link metadata")
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn request_logs_are_created_owner_only_and_existing_logs_are_tightened() {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let created = temp_dir.path().join("fresh").join("requests.jsonl");
        append_request_record_at(&created, &json!({"path": "/health", "status": 200}))
            .expect("append new log");

        let file_mode = fs::metadata(&created)
            .expect("new log metadata")
            .permissions()
            .mode()
            & 0o777;
        let dir_mode = fs::metadata(created.parent().expect("log parent"))
            .expect("new log directory metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "new request log mode was {file_mode:o}");
        assert_eq!(
            dir_mode, 0o700,
            "new request log directory mode was {dir_mode:o}"
        );

        let existing = temp_dir.path().join("existing.jsonl");
        fs::write(&existing, "").expect("seed existing log");
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o644))
            .expect("widen existing log");
        append_request_record_at(&existing, &json!({"path": "/health", "status": 200}))
            .expect("append existing log");
        let existing_mode = fs::metadata(&existing)
            .expect("existing log metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            existing_mode, 0o600,
            "existing request log mode stayed {existing_mode:o}"
        );
    }
}
