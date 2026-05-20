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
            "failed to read generated dapp API client at {}: {error}",
            client_path.display()
        )
    });
    let entries = operation_entries(&source);
    assert!(
        !entries.is_empty(),
        "failed to derive operation paths from generated dapp API client at {}",
        client_path.display()
    );

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR must be set");
    let out_path = Path::new(&out_dir).join("generated_operation_paths.rs");
    fs::write(out_path, generated_table(&entries)).expect("failed to write operation path table");
}

fn operation_entries(source: &str) -> Vec<(String, String, String)> {
    let mut entries = Vec::new();
    let mut pending_route: Option<(String, String)> = None;

    for line in source.lines() {
        if let Some(route) = sends_line_route(line) {
            pending_route = Some(route);
            continue;
        }

        let Some(operation_id) = operation_function_name(line) else {
            continue;
        };
        let Some((method, path)) = pending_route.take() else {
            continue;
        };
        if method_variant(&method).is_some() {
            entries.push((operation_id, method, path));
        }
    }

    entries
}

fn sends_line_route(line: &str) -> Option<(String, String)> {
    let (_, rest) = line.trim().split_once("Sends a `")?;
    let (method, rest) = rest.split_once("` request to `")?;
    let (path, _) = rest.split_once('`')?;
    Some((method.to_string(), path.to_string()))
}

fn operation_function_name(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix("pub async fn ")?;
    let end = rest.find(['<', '('])?;
    Some(rest[..end].to_string())
}

fn generated_table(entries: &[(String, String, String)]) -> String {
    let mut output =
        String::from("const GENERATED_OPERATION_PATHS: &[(&str, HttpMethod, &str)] = &[\n");
    for (operation_id, method, path) in entries {
        let variant = method_variant(method).expect("entries contain supported HTTP methods");
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
