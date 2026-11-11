use crate::config::CliConfig;
use pcl_common::args::CliArgs;
use serde_json::Value;
use std::{
    env,
    fs::{
        self,
        OpenOptions,
    },
    io::{
        BufRead,
        BufReader,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
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
/// route the runner actually sends: `Url::set_path` resolves `.`/`..` (also
/// `%2e`), turns `\` into `/` and drops tabs and newlines. Names match
/// case-insensitively across empty segments. The percent-decoded form also
/// covers servers that decode `%2F` or letters before routing.
pub(crate) fn redact_sensitive_request_path(path: &str) -> String {
    let split_at = path.find(['?', '#']).unwrap_or(path.len());
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

fn redact_invitation_tokens(path: &str) -> Option<String> {
    let mut segments = path.split('/').collect::<Vec<_>>();
    let filled = (0..segments.len())
        .filter(|&index| !segments[index].is_empty())
        .collect::<Vec<_>>();
    let tokens = filled
        .windows(3)
        .filter(|window| {
            is_route_name(segments[window[0]], "invitations")
                && ["preview", "accept"]
                    .iter()
                    .any(|name| is_route_name(segments[window[2]], name))
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

// Also matches `preview;params` or `preview.json`: some servers strip those before routing.
fn is_route_name(segment: &str, name: &str) -> bool {
    segment
        .get(..name.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(name))
        && !segment[name.len()..].starts_with(|c: char| c.is_ascii_alphanumeric())
}

fn redact_record_path(record: &Value) -> Value {
    let Some(path) = record.get("path").and_then(Value::as_str) else {
        return record.clone();
    };
    let redacted = redact_sensitive_request_path(path);
    if redacted == path {
        return record.clone();
    }
    let mut record = record.clone();
    if let Some(object) = record.as_object_mut() {
        object.insert("path".to_string(), Value::String(redacted));
    }
    record
}

fn open_request_log(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{
            OpenOptionsExt,
            PermissionsExt,
        };
        options.mode(0o600);
        let file = options.open(path)?;
        // fchmod the opened file, never a device such as /dev/stderr. A failure
        // fails closed: no record goes to a log we could not restrict.
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

pub fn append_request_record_at(path: &Path, record: &Value) -> std::io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        // Only missing directories are created, each owner-only from the start.
        let mut dirs = fs::DirBuilder::new();
        dirs.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            dirs.mode(0o700);
        }
        dirs.create(parent)?;
    }

    let record = redact_record_path(record);
    let mut bytes = serde_json::to_vec(&record)?;
    bytes.push(b'\n');

    let mut file = open_request_log(path)?;
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

    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut records = reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .map(|record| redact_record_path(&record))
        .collect::<Vec<_>>();

    if records.len() > limit {
        records = records.split_off(records.len() - limit);
    }
    Ok(records)
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
            "/invitations/{t}/preview#frag",
            "/api/v1/invitations/{t}/preview",
            "/x/../invitations/{t}/accept",
            "/invitations/{t}/<redacted>/../preview",
            "/invitations/{t}%2Fpreview",
            "/invitations/{t}%2faccept",
            "/invitations/{t}/pre%76iew",
            "/invitations/{t}%2F..%2Fx/preview",
            "/invitations/{t}/preview;x",
            "/invitations/{t}/preview%23x",
            "/invitations;v=1/{t}/accept.json",
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
