use serde_json::{
    Map,
    Value,
    json,
};
use std::collections::BTreeSet;

const API_ERROR_SCHEMA_NAME: &str = "ApiError";
const API_ERROR_SCHEMA_REF: &str = "#/components/schemas/ApiError";
const API_MESSAGE_ERROR_SCHEMA_NAME: &str = "ApiMessageError";
const API_MESSAGE_ERROR_SCHEMA_REF: &str = "#/components/schemas/ApiMessageError";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ComponentRef {
    section: String,
    name: String,
}

pub(crate) fn retain_client_operations(spec: &mut Value, operation_ids: &[&str]) {
    let operation_ids: BTreeSet<&str> = operation_ids.iter().copied().collect();
    retain_paths(spec, &operation_ids);
    prune_components(spec);
}

pub(crate) fn normalize_error_response_schemas(spec: &mut Value) {
    ensure_api_error_schema(spec);

    let Some(paths) = spec.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };

    for path_item in paths.values_mut() {
        let Some(path_item) = path_item.as_object_mut() else {
            continue;
        };
        for (method, operation) in path_item {
            if !is_http_method(method) {
                continue;
            }
            let Some(operation) = operation.as_object_mut() else {
                continue;
            };
            let Some(responses) = operation
                .get_mut("responses")
                .and_then(Value::as_object_mut)
            else {
                continue;
            };
            for (status, response) in responses {
                if !is_error_status(status) {
                    continue;
                }
                rewrite_json_error_response(response);
            }
        }
    }
}

fn ensure_api_error_schema(spec: &mut Value) {
    if !spec.is_object() {
        *spec = json!({});
    }
    let root = spec.as_object_mut().expect("spec object was just created");
    let components = root
        .entry("components")
        .or_insert_with(|| Value::Object(Map::new()));
    if !components.is_object() {
        *components = Value::Object(Map::new());
    }
    let components = components
        .as_object_mut()
        .expect("components object was just created");
    let schemas = components
        .entry("schemas")
        .or_insert_with(|| Value::Object(Map::new()));
    if !schemas.is_object() {
        *schemas = Value::Object(Map::new());
    }
    let schemas = schemas
        .as_object_mut()
        .expect("schemas object was just created");
    schemas
        .entry(API_ERROR_SCHEMA_NAME)
        .or_insert_with(api_error_schema);
    schemas
        .entry(API_MESSAGE_ERROR_SCHEMA_NAME)
        .or_insert_with(api_message_error_schema);
}

fn api_error_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "error": {
                "type": "string"
            },
            "code": {
                "type": "string"
            },
            "details": {
                "type": "string"
            }
        },
        "required": ["error"]
    })
}

fn api_message_error_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "code": {
                "type": "string"
            },
            "message": {
                "type": "string"
            }
        },
        "required": ["code", "message"]
    })
}

fn rewrite_json_error_response(response: &mut Value) {
    let Some(content) = response.get_mut("content").and_then(Value::as_object_mut) else {
        return;
    };
    for (media_type, media) in content {
        if !is_json_media_type(media_type) {
            continue;
        }
        let Some(schema) = media.get_mut("schema") else {
            continue;
        };
        if let Some(schema_ref) = standard_error_schema_ref(schema) {
            *schema = json!({ "$ref": schema_ref });
        }
    }
}

fn standard_error_schema_ref(schema: &Value) -> Option<&'static str> {
    if is_error_code_details_schema(schema) {
        Some(API_ERROR_SCHEMA_REF)
    } else if is_code_message_schema(schema) {
        Some(API_MESSAGE_ERROR_SCHEMA_REF)
    } else {
        None
    }
}

fn is_error_code_details_schema(schema: &Value) -> bool {
    let Some(schema) = schema.as_object() else {
        return false;
    };
    if schema.contains_key("$ref") || schema.get("type").and_then(Value::as_str) != Some("object") {
        return false;
    }
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let Some(error_schema) = properties.get("error") else {
        return false;
    };
    if !is_string_schema(error_schema) || !requires_error(schema) {
        return false;
    }
    properties.iter().all(|(name, property)| {
        matches!(name.as_str(), "error" | "code" | "details") && is_string_schema(property)
    })
}

fn is_code_message_schema(schema: &Value) -> bool {
    let Some(schema) = schema.as_object() else {
        return false;
    };
    if schema.contains_key("$ref") || schema.get("type").and_then(Value::as_str) != Some("object") {
        return false;
    }
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return false;
    };
    let required = schema.get("required").and_then(Value::as_array);
    if !required.is_some_and(|required| {
        required.iter().any(|value| value.as_str() == Some("code"))
            && required
                .iter()
                .any(|value| value.as_str() == Some("message"))
    }) {
        return false;
    }
    properties.iter().all(|(name, property)| {
        matches!(name.as_str(), "code" | "message") && is_string_schema(property)
    })
}

fn is_string_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("string")
}

fn requires_error(schema: &Map<String, Value>) -> bool {
    schema
        .get("required")
        .and_then(Value::as_array)
        .is_some_and(|required| required.iter().any(|value| value.as_str() == Some("error")))
}

fn is_error_status(status: &str) -> bool {
    status == "default"
        || status
            .parse::<u16>()
            .is_ok_and(|status| (400..=599).contains(&status))
}

fn is_json_media_type(media_type: &str) -> bool {
    media_type == "application/json" || media_type.starts_with("application/json;")
}

fn is_http_method(method: &str) -> bool {
    matches!(
        method,
        "get" | "post" | "put" | "delete" | "patch" | "head" | "options"
    )
}

fn retain_paths(spec: &mut Value, operation_ids: &BTreeSet<&str>) {
    let Some(paths) = spec.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };

    paths.retain(|_, path_item| {
        let Some(path_item) = path_item.as_object_mut() else {
            return false;
        };
        path_item.retain(|method, operation| {
            !is_http_method(method)
                || operation
                    .get("operationId")
                    .and_then(Value::as_str)
                    .is_some_and(|operation_id| operation_ids.contains(operation_id))
        });
        path_item.keys().any(|method| is_http_method(method))
    });
}

fn prune_components(spec: &mut Value) {
    let mut required = BTreeSet::new();
    collect_refs(spec.get("paths").unwrap_or(&Value::Null), &mut required);

    loop {
        let before = required.len();
        for component_ref in required.clone() {
            if let Some(component) = component_value(spec, &component_ref) {
                collect_refs(component, &mut required);
            }
        }
        if required.len() == before {
            break;
        }
    }

    let Some(components) = spec.get_mut("components").and_then(Value::as_object_mut) else {
        return;
    };
    components.retain(|section, entries| {
        let Some(entries) = entries.as_object_mut() else {
            return false;
        };
        entries.retain(|name, _| {
            required.contains(&ComponentRef {
                section: section.clone(),
                name: name.clone(),
            })
        });
        !entries.is_empty()
    });
}

fn component_value<'a>(spec: &'a Value, component_ref: &ComponentRef) -> Option<&'a Value> {
    spec.get("components")?
        .get(&component_ref.section)?
        .get(&component_ref.name)
}

fn collect_refs(value: &Value, refs: &mut BTreeSet<ComponentRef>) {
    match value {
        Value::Object(map) => {
            if let Some(component_ref) = map.get("$ref").and_then(Value::as_str).and_then(parse_ref)
            {
                refs.insert(component_ref);
            }
            for value in map.values() {
                collect_refs(value, refs);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_refs(value, refs);
            }
        }
        _ => {}
    }
}

fn parse_ref(reference: &str) -> Option<ComponentRef> {
    let reference = reference.strip_prefix("#/components/")?;
    let (section, name) = reference.split_once('/')?;
    Some(ComponentRef {
        section: unescape_json_pointer(section),
        name: unescape_json_pointer(name),
    })
}

fn unescape_json_pointer(value: &str) -> String {
    value.replace("~1", "/").replace("~0", "~")
}
