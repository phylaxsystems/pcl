use crate::config::CliConfig;
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
    path::PathBuf,
};

const REQUEST_LOG_ENV: &str = "PCL_REQUEST_LOG";
const REQUEST_LOG_FILE: &str = "requests.jsonl";

pub fn request_log_path() -> PathBuf {
    env::var_os(REQUEST_LOG_ENV).map_or_else(
        || CliConfig::get_config_dir().join(REQUEST_LOG_FILE),
        PathBuf::from,
    )
}

pub fn append_request_record(record: &Value) -> std::io::Result<()> {
    let path = request_log_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    serde_json::to_writer(&mut file, record)?;
    file.write_all(b"\n")?;
    Ok(())
}

pub fn read_request_records(limit: usize) -> std::io::Result<Vec<Value>> {
    let path = request_log_path();
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
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(path)?;
    Ok(true)
}
