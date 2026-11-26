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
    // Same exclusive lock as a migrating read, so an append cannot land in
    // the middle of an in-place rewrite.
    if file.metadata()?.is_file() {
        file.lock()?;
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
    if request_log_is_rewritable(path)? {
        return read_and_migrate_request_log(path, limit);
    }
    read_request_window(path, limit)
}

/// A regular file can be rewritten in place. Symlinks and devices are left
/// untouched: rewriting a symlink would either follow it or replace it.
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
    let file = fs::File::open(path)?;
    if limit == 0 {
        return Ok(Vec::new());
    }
    let MigratedLog { records, .. } = scan_request_log(BufReader::new(file), limit)?;
    Ok(records)
}

/// Removes invitation tokens from the whole log, including lines outside the
/// returned window. A later append must not keep the old bytes.
fn read_and_migrate_request_log(path: &Path, limit: usize) -> std::io::Result<Vec<Value>> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .map_err(|error| map_request_log_open_error(path, error))?;
    file.lock()?;

    let migrated = {
        let reader = BufReader::new(&file);
        scan_request_log(reader, limit)?
    };
    if let Some(bytes) = migrated.rewritten {
        // Truncate before writing so a crash cannot leave the old token tail
        // behind a shorter redacted line.
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        #[cfg(unix)]
        if file.metadata()?.is_file() {
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
    }
    Ok(migrated.records)
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

struct MigratedLog {
    records: Vec<Value>,
    rewritten: Option<Vec<u8>>,
}

fn scan_request_log<R: BufRead>(reader: R, limit: usize) -> std::io::Result<MigratedLog> {
    let mut records = VecDeque::new();
    let mut rewritten = Vec::new();
    let mut changed = false;
    for line in reader.lines() {
        let Ok(line) = line else {
            // A partial scan must not replace the rest of the file.
            return Ok(MigratedLog {
                records: Vec::from(records),
                rewritten: None,
            });
        };
        let (stored, line_changed) = migrated_request_line(&line)?;
        changed |= line_changed;
        if limit > 0
            && let Ok(record) = serde_json::from_str::<Value>(&stored)
        {
            if records.len() == limit {
                records.pop_front();
            }
            records.push_back(record);
        }
        rewritten.extend_from_slice(stored.as_bytes());
        rewritten.push(b'\n');
    }
    Ok(MigratedLog {
        records: Vec::from(records),
        rewritten: changed.then_some(rewritten),
    })
}

fn migrated_request_line(line: &str) -> std::io::Result<(String, bool)> {
    let Ok(mut record) = serde_json::from_str::<Value>(line) else {
        return Ok((line.to_string(), false));
    };
    let Some(path) = record.get("path").and_then(Value::as_str) else {
        return Ok((line.to_string(), false));
    };
    let redacted = redact_sensitive_request_path(path);
    if redacted == path {
        return Ok((line.to_string(), false));
    }
    let Some(object) = record.as_object_mut() else {
        return Ok((line.to_string(), false));
    };
    object.insert("path".to_string(), Value::String(redacted));
    let stored = serde_json::to_string(&record)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok((stored, true))
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
