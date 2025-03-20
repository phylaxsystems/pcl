use std::path::PathBuf;

/// Reads a contract artifact
/// Input can be specified in two patterns
/// 1. ${file_name.sol}:${contract_name}
/// 2. ${contract_name} (file_name is assumed to be the same as contract_name, with .sol extension)
///
/// out_dir is the output directory of the build artifact
pub fn read_artifact(input: &str, out_dir: &PathBuf) -> serde_json::Value {
    let mut parts = input.split(':');

    let contract_name;
    let file_name;
    if parts.clone().count() > 1 {
        file_name = parts.next().expect("Failed to read file name").to_string();
        contract_name = parts.next().expect("Failed to read contract name");
    } else {
        contract_name = parts.next().expect("Failed to read contract name");
        let mut path = PathBuf::from(contract_name);
        path.set_extension("sol");
        file_name = path.to_string_lossy().to_string();
    }

    let new_path = out_dir.join(format!("{}/{}.json", file_name, contract_name));

    let file = std::fs::File::open(new_path).expect("Failed to open file");
    serde_json::from_reader(file).expect("Failed to parse JSON")
}

/// Reads deployment bytecode from a contract artifact
/// Input can be specified in two patterns
/// 1. ${file_name.sol}:${contract_name}
/// 2. ${contract_name} (file_name is assumed to be the same as contract_name, with .sol extension)
///
/// out_dir is the output directory of the build artifact
pub fn bytecode(input: &str, out_dir: &PathBuf) -> String {
    let value = read_artifact(input, out_dir);
    let bytecode = value["bytecode"]["object"]
        .as_str()
        .expect("Failed to read bytecode");
    bytecode.to_string()
}

pub fn compilation_target(input: &str, out_dir: &PathBuf) -> String {
    let value = read_artifact(input, out_dir);
    // The compilationTarget is a map with a single key-value pair where the key is the file path
    // and the value is the contract name. We need to extract the file path (key).
    let compilation_target = value["metadata"]["settings"]["compilationTarget"]
        .as_object()
        .expect("Failed to read compilation target as object");
    // Get the compilation target of the contract with name contract_name
    compilation_target
        .iter()
        .find_map(|(key, value)| {
            if value.as_str() == Some(input) {
                Some(key.to_string())
            } else {
                None
            }
        })
        .expect("Failed to find contract in compilation target")
}

pub fn compiler_version(input: &str, out_dir: &PathBuf) -> String {
    let value = read_artifact(input, out_dir);
    let compiler_version = value["metadata"]["compiler"]["version"]
        .as_str()
        .expect("failed to read compiler version");
    compiler_version.to_string()
}
