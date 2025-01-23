fn read_artifact(input: &str) -> serde_json::Value {
    let mut parts = input.split(':');
    let file_name = parts.next().expect("Failed to read filename");
    let contract_name = parts.next().expect("Failed to read contract name");
    let path = format!("contract-mocks/out/{}/{}.json", file_name, contract_name);

    let file = std::fs::File::open(path).expect("Failed to open file");
    serde_json::from_reader(file).expect("Failed to parse JSON")
}

/// Reads deployment bytecode from a contract-mocks artifact
///
/// # Arguments
/// * `input` - ${file_name}:${contract_name}
pub fn bytecode(input: &str) -> String {
    let value = read_artifact(input);
    let bytecode = value["bytecode"]["object"]
        .as_str()
        .expect("Failed to read bytecode");
    bytecode.to_string()
}
