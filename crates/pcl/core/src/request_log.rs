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

pub fn append_request_record_at(path: &Path, record: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut bytes = serde_json::to_vec(record)?;
    bytes.push(b'\n');

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
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
