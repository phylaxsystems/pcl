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
}
