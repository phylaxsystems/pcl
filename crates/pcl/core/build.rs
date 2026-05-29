use std::{
    env,
    fmt::Write as _,
    fs,
    path::Path,
};

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR must be set");
    let client_path =
        Path::new(&manifest_dir).join("../../dapp-api-client/src/generated/client.rs");
    println!("cargo:rerun-if-changed={}", client_path.display());

    let source = fs::read_to_string(&client_path).unwrap_or_else(|error| {
        panic!(
            "failed to read generated OpenAPI client at {}: {error}",
            client_path.display()
        )
    });

    let entries = operation_entries(&source);
    assert!(
        !entries.is_empty(),
        "failed to derive operation paths from generated OpenAPI client at {}",
        client_path.display()
    );

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR must be set");
    let out_path = Path::new(&out_dir).join("generated_operation_paths.rs");
    fs::write(out_path, generated_table(&entries)).expect("failed to write operation path table");
}

fn operation_entries(source: &str) -> Vec<(String, String, String)> {
    let mut entries = Vec::new();
    let mut pending_request: Option<(String, String)> = None;

    for line in source.lines() {
        if let Some((method, path)) = parse_request_doc_line(line) {
            pending_request = Some((method.to_string(), path.to_string()));
            continue;
        }

        let Some(operation_id) = parse_operation_id_line(line) else {
            continue;
        };
        let Some((method, path)) = pending_request.take() else {
            continue;
        };
        let Some(variant) = method_variant(&method) else {
            continue;
        };
        entries.push((operation_id.to_string(), variant.to_string(), path));
    }

    entries
}

fn parse_request_doc_line(line: &str) -> Option<(&str, &str)> {
    let remainder = line.trim().strip_prefix("Sends a `")?;
    let (method, remainder) = remainder.split_once('`')?;
    let remainder = remainder.strip_prefix(" request to `")?;
    let (path, _) = remainder.split_once('`')?;
    Some((method, path))
}

fn parse_operation_id_line(line: &str) -> Option<&str> {
    let remainder = line.trim().strip_prefix("operation_id: \"")?;
    let (operation_id, _) = remainder.split_once('"')?;
    Some(operation_id)
}

fn generated_table(entries: &[(String, String, String)]) -> String {
    let mut output =
        String::from("const GENERATED_OPERATION_PATHS: &[(&str, HttpMethod, &str)] = &[\n");
    for (operation_id, variant, path) in entries {
        writeln!(
            output,
            "    ({operation_id:?}, HttpMethod::{variant}, {path:?}),"
        )
        .expect("writing generated operation table cannot fail");
    }
    output.push_str("];\n");
    output
}

fn method_variant(method: &str) -> Option<&'static str> {
    match method {
        "GET" => Some("Get"),
        "POST" => Some("Post"),
        "PUT" => Some("Put"),
        "PATCH" => Some("Patch"),
        "DELETE" => Some("Delete"),
        _ => None,
    }
}
