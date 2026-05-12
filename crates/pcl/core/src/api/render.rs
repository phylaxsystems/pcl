use super::{
    ApiCommandError,
    first_string_field,
    with_envelope_metadata,
};
use pcl_common::args::{
    OutputMode,
    current_output_mode,
};
use serde_json::Value;
use std::fmt::Write as _;

pub(super) fn print_output(value: &Value, json_output: bool) -> Result<(), ApiCommandError> {
    print!("{}", envelope_output_string(value, json_output)?);
    Ok(())
}

pub fn envelope_output_string(
    value: &Value,
    json_output: bool,
) -> Result<String, serde_json::Error> {
    let value = with_envelope_metadata(value.clone());
    let output_mode = if json_output {
        OutputMode::Json
    } else {
        current_output_mode()
    };
    match output_mode {
        OutputMode::Json => Ok(format!("{}\n", serde_json::to_string_pretty(&value)?)),
        OutputMode::Toon => Ok(toon_string(&value)),
        OutputMode::Human => Ok(human_string(&value)),
    }
}

/// Render an envelope for interactive humans.
pub fn human_string(value: &Value) -> String {
    let value = with_envelope_metadata(value.clone());
    let status = value.get("status").and_then(Value::as_str).unwrap_or("ok");
    let mut output = String::new();
    output.push_str(match status {
        "ok" => "OK",
        "error" => "Error",
        "action_required" => "Action required",
        "pending" => "Pending",
        other => other,
    });
    output.push('\n');

    if let Some(error) = value.get("error") {
        render_human_error(&mut output, error);
    } else if !render_human_special(&mut output, &value)
        && !render_human_collection(&mut output, &value)
        && let Some(data) = value.get("data")
    {
        render_human_summary(&mut output, data);
    }

    let human_actions = human_next_actions(&value);
    if !human_actions.is_empty() {
        output.push_str("\nNext:\n");
        for (index, action) in human_actions.iter().enumerate() {
            output.push_str("  ");
            output.push_str(&(index + 1).to_string());
            output.push_str(". ");
            output.push_str(action);
            output.push('\n');
        }
    }
    render_human_request_id(&mut output, &value);
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

fn human_next_actions(envelope: &Value) -> Vec<String> {
    let status = envelope
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("ok");
    let is_empty_ok = status == "ok" && envelope_has_empty_results(envelope);
    let terms_accepted = envelope_terms_accepted(envelope);
    let preserve_agent_flags = envelope
        .get("data")
        .and_then(|data| data.get("consumption_order"))
        .is_some();
    let integration_test_unavailable = envelope
        .pointer("/data/test_available")
        .or_else(|| envelope.pointer("/data/data/test_available"))
        .and_then(Value::as_bool)
        == Some(false);
    envelope
        .get("next_actions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|action| !is_dangerous_or_internal_action(action))
        .filter(|action| !(is_empty_ok && is_item_placeholder_action(action)))
        .filter(|action| !(terms_accepted && action.contains("account --accept-terms")))
        .filter(|action| !(integration_test_unavailable && action.contains(" --test")))
        .map(|action| {
            if preserve_agent_flags {
                action.to_string()
            } else {
                human_action_str(action)
            }
        })
        .filter(|action| !action.is_empty())
        .collect()
}

fn envelope_terms_accepted(envelope: &Value) -> bool {
    envelope
        .pointer("/data/terms_accepted")
        .or_else(|| envelope.pointer("/data/data/terms_accepted"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn is_dangerous_or_internal_action(action: &str) -> bool {
    action.contains(" config delete")
        || action.contains(" --delete")
        || action.contains(" --remove")
        || action.contains(" --revoke")
        || action.contains(" --logout")
        || action.starts_with("Read error.http.body")
        || action.starts_with("Use data.")
}

fn is_item_placeholder_action(action: &str) -> bool {
    [
        "<assertion-id>",
        "<incident-id>",
        "<release-id>",
        "<transfer-id>",
        "<adopter-id>",
        "<job-id>",
        "<project-ref>",
        "<contract-ref>",
        "<token>",
    ]
    .iter()
    .any(|placeholder| action.contains(placeholder))
}

fn envelope_has_empty_results(envelope: &Value) -> bool {
    let Some(data) = envelope.get("data") else {
        return false;
    };
    value_has_empty_results(data)
}

fn value_has_empty_results(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.is_empty(),
        Value::Object(object) => {
            if let Some(inner) = object.get("data")
                && value_has_empty_results(inner)
            {
                return true;
            }
            object.iter().any(|(key, value)| {
                !key.starts_with('_')
                    && (value.as_array().is_some_and(Vec::is_empty)
                        || value_has_empty_results(value))
            })
        }
        _ => false,
    }
}

struct HumanCollection<'a> {
    field: String,
    name: String,
    items: &'a [Value],
    pagination: Option<&'a Value>,
    meta: Option<&'a Value>,
}

fn render_human_error(output: &mut String, error: &Value) {
    output.push('\n');
    let code = error.get("code").and_then(Value::as_str);
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        output.push_str(&human_error_message(code, message));
        output.push('\n');
    } else if let Some(error) = error.as_str() {
        output.push_str(error);
        output.push('\n');
    } else {
        render_human_value(output, error, 0);
    }

    if let Some(reason) = api_error_reason(error) {
        output.push_str("API reason: ");
        output.push_str(&reason);
        output.push('\n');
    }
    if let Some(request_id) = error.get("request_id").and_then(Value::as_str) {
        output.push_str("Request ID: ");
        output.push_str(request_id);
        output.push('\n');
    }
}

fn human_error_message(code: Option<&str>, message: &str) -> String {
    if code.is_some_and(|value| value.starts_with("cli.")) {
        return clean_cli_error_message(message);
    }
    match code {
        Some("api.not_found") => {
            "Resource not found. Check the ID, slug, or API path and try again.".to_string()
        }
        Some("network.request_failed") => {
            "Network request failed. Check --api-url and your network connection, then retry."
                .to_string()
        }
        Some("api.server_error") => {
            "The platform returned a server error. Retry later or report the request ID."
                .to_string()
        }
        _ => message.to_string(),
    }
}

fn clean_cli_error_message(message: &str) -> String {
    let lines = message
        .lines()
        .take_while(|line| !line.starts_with("Usage:") && !line.starts_with("For more information"))
        .map(|line| line.strip_prefix("error: ").unwrap_or(line).trim_end())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.first() == Some(&"the following required arguments were not provided:")
        && let Some(argument) = lines.get(1)
    {
        return format!("Missing required argument: {}", argument.trim());
    }
    lines.join("\n")
}

fn api_error_reason(error: &Value) -> Option<String> {
    let body = error.pointer("/http/body")?;
    for key in ["message", "error", "detail", "reason"] {
        if let Some(value) = body.get(key).and_then(Value::as_str)
            && !value.is_empty()
        {
            return Some(value.to_string());
        }
    }
    body.as_str().map(ToString::to_string)
}

fn render_human_special(output: &mut String, envelope: &Value) -> bool {
    let Some(data) = envelope.get("data") else {
        return false;
    };
    let display_data = data.get("data").unwrap_or(data);

    for render in [
        render_login_challenge as fn(&mut String, &Value) -> bool,
        render_request_plan,
        render_auth_status,
        render_identity_status,
        render_doctor,
    ] {
        if render(output, display_data) {
            return true;
        }
    }
    if render_project_home(output, data, display_data) {
        return true;
    }
    for render in [
        render_project_detail as fn(&mut String, &Value) -> bool,
        render_incident_detail,
        render_search_results,
        render_account_detail,
        render_deployment_state,
        render_transfer_state,
        render_integration_status,
        render_protocol_manager_status,
    ] {
        if render(output, display_data) {
            return true;
        }
    }
    if render_mutation_success(output, envelope, display_data) {
        return true;
    }
    for render in [
        render_api_manifest as fn(&mut String, &Value) -> bool,
        render_llms_guide,
        render_workflow_detail,
        render_schema_detail,
        render_operation_detail,
        render_api_coverage,
        render_raw_api_response,
        render_export_result,
        render_job_detail,
        render_path_or_toggle_result,
    ] {
        if render(output, display_data) {
            return true;
        }
    }
    if render_body_template(output, envelope, display_data) {
        return true;
    }

    false
}

fn render_login_challenge(output: &mut String, data: &Value) -> bool {
    if data.get("state").and_then(Value::as_str) != Some("login_required") {
        return false;
    }
    output.push_str("\nLogin required\n");
    if let Some(reason) = data.get("reason").and_then(Value::as_str) {
        writeln!(output, "Reason: {}", human_label(reason)).expect("write to string");
    }
    if let Some(url) = data.get("device_url").and_then(Value::as_str) {
        writeln!(output, "Open: {url}").expect("write to string");
    }
    if let Some(code) = data.get("code").and_then(Value::as_str) {
        writeln!(output, "Code: {code}").expect("write to string");
    }
    if let Some(expires_at) = data.get("expires_at").and_then(Value::as_str) {
        writeln!(output, "Expires: {}", format_timestamp(expires_at)).expect("write to string");
    }
    if let Some(command) = data.get("poll_command").and_then(Value::as_str) {
        writeln!(output, "Poll: {}", humanize_command(command)).expect("write to string");
    }
    true
}

fn render_request_plan(output: &mut String, data: &Value) -> bool {
    if data.get("dry_run").and_then(Value::as_bool) != Some(true) {
        return false;
    }

    output.push_str("\nDry run\n");
    if data.get("valid").and_then(Value::as_bool) == Some(false) {
        output.push_str("Request is not valid.\n");
        if let Some(error) = data.get("error") {
            render_human_error(output, error);
        }
        return true;
    }

    let request = data.get("request").unwrap_or(data);
    let method = request.get("method").and_then(Value::as_str).unwrap_or("-");
    let path = request.get("path").and_then(Value::as_str).unwrap_or("-");
    writeln!(output, "{method} {path}").expect("write to string");
    if let Some(query) = request.get("query").and_then(Value::as_array)
        && !query.is_empty()
    {
        output.push_str("Query: ");
        output.push_str(&name_value_pairs(query));
        output.push('\n');
    }
    if let Some(auth) = request.get("auth") {
        let required = auth
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let attached = auth
            .get("will_attach_stored_token")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        writeln!(
            output,
            "Auth: {}{}",
            if required { "required" } else { "not required" },
            if attached {
                ", stored token will be attached"
            } else {
                ""
            }
        )
        .expect("write to string");
    }
    if let Some(body) = request.get("body")
        && !body.is_null()
    {
        output.push_str("Body: ");
        output.push_str(&human_compact_summary(body));
        output.push('\n');
    }
    if let Some(pagination) = data.get("pagination")
        && !pagination.is_null()
    {
        output.push_str("Pagination: ");
        output.push_str(&human_compact_summary(pagination));
        output.push('\n');
    }
    true
}

fn render_auth_status(output: &mut String, data: &Value) -> bool {
    if !data.get("authenticated").is_some_and(Value::is_boolean)
        || data.get("auth").is_some()
        || data.get("config_path").is_some()
    {
        return false;
    }

    output.push_str("\nAuthentication\n");
    let authenticated = data
        .get("authenticated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    writeln!(
        output,
        "Status: {}",
        if authenticated {
            "authenticated"
        } else {
            "not logged in"
        }
    )
    .expect("write to string");
    if let Some(user) = data.get("user").and_then(Value::as_str) {
        writeln!(output, "User: {user}").expect("write to string");
    }
    if let Some(email) = data.get("email").and_then(Value::as_str)
        && data.get("user").and_then(Value::as_str) != Some(email)
    {
        writeln!(output, "Email: {email}").expect("write to string");
    }
    if let Some(wallet) = data.get("wallet_address").and_then(Value::as_str) {
        writeln!(output, "Wallet: {wallet}").expect("write to string");
    }
    if let Some(expires_at) = data.get("expires_at").and_then(Value::as_str) {
        writeln!(output, "Token expires: {}", format_timestamp(expires_at))
            .expect("write to string");
    }
    if let Some(seconds) = data.get("seconds_remaining").and_then(Value::as_i64) {
        writeln!(output, "Time remaining: {}", format_duration(seconds)).expect("write to string");
    }
    if data.get("refreshed").and_then(Value::as_bool) == Some(true) {
        output.push_str("Token refreshed.\n");
    }
    if let Some(request_id) = data.get("request_id").and_then(Value::as_str) {
        writeln!(output, "Request ID: {request_id}").expect("write to string");
    }
    true
}

fn render_identity_status(output: &mut String, data: &Value) -> bool {
    let Some(auth) = data.get("auth") else {
        return false;
    };
    if !auth.get("authenticated").is_some_and(Value::is_boolean) {
        return false;
    }
    output.push_str("\nIdentity\n");
    let authenticated = auth
        .get("authenticated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    writeln!(
        output,
        "Status: {}",
        if authenticated {
            "authenticated"
        } else {
            "not logged in"
        }
    )
    .expect("write to string");
    if let Some(user) = auth.get("user").and_then(Value::as_str) {
        writeln!(output, "User: {user}").expect("write to string");
    }
    if let Some(user_id) = auth.get("user_id").and_then(Value::as_str) {
        writeln!(output, "User ID: {user_id}").expect("write to string");
    }
    if let Some(expires_at) = auth.get("expires_at").and_then(Value::as_str) {
        writeln!(output, "Token expires: {}", format_timestamp(expires_at))
            .expect("write to string");
    }
    if let Some(config_path) = data.get("config_path").and_then(Value::as_str) {
        writeln!(output, "Config: {config_path}").expect("write to string");
    }
    if data.get("offline").and_then(Value::as_bool) == Some(true) {
        output.push_str("Network checks skipped.\n");
    }
    true
}

fn render_doctor(output: &mut String, data: &Value) -> bool {
    let Some(checks) = data.get("checks").and_then(Value::as_array) else {
        return false;
    };
    output.push_str("\nDoctor\n");
    render_checks_table(output, checks);
    if let Some(api_url) = data.get("api_url").and_then(Value::as_str) {
        writeln!(output, "\nAPI: {api_url}").expect("write to string");
    }
    output.push_str("Default output: human. Agents should pass --toon; scripts can pass --json.\n");
    true
}

fn render_project_detail(output: &mut String, data: &Value) -> bool {
    if data.get("project_id").is_none() || data.get("project_name").is_none() {
        return false;
    }
    output.push_str("\nProject\n");
    write_string_field(output, "Name", data, "project_name");
    write_string_field(output, "ID", data, "project_id");
    write_string_field(output, "Slug", data, "slug");
    if let Some(private) = data.get("is_private").and_then(Value::as_bool) {
        writeln!(
            output,
            "Visibility: {}",
            if private { "private" } else { "public" }
        )
        .expect("write to string");
    }
    if let Some(dev) = data.get("is_dev").and_then(Value::as_bool) {
        writeln!(
            output,
            "Mode: {}",
            if dev { "development" } else { "production" }
        )
        .expect("write to string");
    }
    write_network_list_for_value(output, data);
    write_optional_string_field(output, "Description", data, "project_description");
    write_optional_string_field(output, "GitHub", data, "github_url");
    write_timestamp_field(output, "Created", data, "created_at");
    write_timestamp_field(output, "Updated", data, "updated_at");
    if let Some(manager) = data
        .get("protocol_manager_address")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        writeln!(output, "Protocol manager: {manager}").expect("write to string");
    } else {
        output.push_str("Protocol manager: not set\n");
    }
    write_count_field(
        output,
        "Submitted assertions",
        data,
        "submitted_assertion_ids",
    );
    write_u64_field(output, "Saved by", data, "saved_count", Some("users"));
    true
}

fn render_incident_detail(output: &mut String, data: &Value) -> bool {
    let Some(incident_id) = data.get("incident_id").and_then(Value::as_str) else {
        return false;
    };
    if data.get("invalidating_transactions").is_none() && data.get("transaction_count").is_none() {
        return false;
    }

    output.push_str("\nIncident\n");
    writeln!(output, "ID: {incident_id}").expect("write to string");
    write_optional_string_field(output, "Reference", data, "public_reference_id");
    write_u64_field(output, "Chain", data, "chain_id", None);
    write_timestamp_field(output, "Window start", data, "window_start");
    write_string_field(output, "Environment", data, "environment");

    if let Some(assertion) = data.get("assertion") {
        output.push_str("\nAssertion\n");
        write_optional_string_field(output, "Title", assertion, "title");
        write_optional_string_field(output, "ID", assertion, "assertion_id");
        if let Some(description) = assertion
            .get("description")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .filter(|value| !is_hex_blob(value))
        {
            writeln!(output, "Description: {}", truncate(description, 96))
                .expect("write to string");
        }
    } else {
        write_optional_string_field(output, "Assertion ID", data, "assertion_id");
    }

    if let Some(adopter) = data.get("assertion_adopter") {
        output.push_str("\nAssertion adopter\n");
        write_optional_string_field(output, "Name", adopter, "name");
        write_optional_string_field(output, "Address", adopter, "address");
        write_optional_string_field(output, "ID", adopter, "id");
    } else {
        write_optional_string_field(output, "Assertion adopter ID", data, "assertion_adopter_id");
    }

    output.push_str("\nTrace summary\n");
    if let Some(value) = data.get("transaction_count").and_then(Value::as_u64) {
        writeln!(
            output,
            "Invalidating transactions: {}",
            plural_count(value, "transaction")
        )
        .expect("write to string");
    }
    write_u64_field(output, "Traces completed", data, "traces_completed", None);
    write_u64_field(output, "Traces pending", data, "traces_pending", None);

    if let Some(transactions) = data
        .get("invalidating_transactions")
        .and_then(Value::as_array)
        .filter(|transactions| !transactions.is_empty())
    {
        let shown = transactions.len().min(5);
        writeln!(
            output,
            "\nInvalidating transactions (first {shown} of {})",
            transactions.len()
        )
        .expect("write to string");
        writeln!(
            output,
            "{} {} {} {} Trace",
            pad("#", 3),
            pad("Time", 16),
            pad("Tx hash", 20),
            pad("Result", 11)
        )
        .expect("write to string");
        for (index, tx) in transactions.iter().take(shown).enumerate() {
            let time = tx
                .get("incident_timestamp")
                .and_then(Value::as_str)
                .map_or_else(|| "-".to_string(), format_timestamp);
            let hash = first_string_field(tx, &["transaction_hash", "hash", "tx_hash"])
                .map_or_else(|| "-".to_string(), |value| truncate(&value, 20));
            let result = match tx.get("landed_on_chain").and_then(Value::as_bool) {
                Some(true) => "landed",
                Some(false) => "invalidated",
                None => "-",
            };
            let trace = tx
                .get("debug_traces")
                .and_then(Value::as_array)
                .and_then(|traces| traces.first())
                .and_then(|trace| trace.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("-");
            writeln!(
                output,
                "{} {} {} {} {}",
                pad(&(index + 1).to_string(), 3),
                pad(&time, 16),
                pad(&hash, 20),
                pad(result, 11),
                trace
            )
            .expect("write to string");
        }
    }

    true
}

fn render_project_home(output: &mut String, envelope_data: &Value, data: &Value) -> bool {
    let Some(member_projects) = data.get("member_projects").and_then(Value::as_array) else {
        return false;
    };
    let saved_projects = data
        .get("saved_projects")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let no_project_adopters = data
        .get("no_project_adopters")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    output.push_str("\nYour projects\n");
    writeln!(
        output,
        "Showing {} you belong to",
        plural_count(member_projects.len(), "project")
    )
    .expect("write to string");
    if let Some(meta) = envelope_data.get("_meta") {
        render_collection_meta(output, meta);
    }
    output.push('\n');

    if member_projects.is_empty() {
        output.push_str("No projects found for your account.\n");
    } else {
        render_projects_table(output, member_projects);
    }

    writeln!(
        output,
        "\nSaved projects: {}",
        plural_count(saved_projects.len(), "project")
    )
    .expect("write to string");
    if !saved_projects.is_empty() {
        render_projects_table(output, saved_projects);
    }
    writeln!(
        output,
        "Contracts without a project: {}",
        plural_count(no_project_adopters.len(), "contract")
    )
    .expect("write to string");
    true
}

fn render_search_results(output: &mut String, data: &Value) -> bool {
    let Some(projects) = data.get("projects").and_then(Value::as_array) else {
        return false;
    };
    let contracts = data
        .get("contracts")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    let assertions = data
        .get("assertions")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    output.push_str("\nSearch results\n");
    writeln!(output, "Projects: {}", projects.len()).expect("write to string");
    writeln!(output, "Contracts: {}", contracts.len()).expect("write to string");
    writeln!(output, "Assertions: {}", assertions.len()).expect("write to string");

    if projects.is_empty() && contracts.is_empty() && assertions.is_empty() {
        output.push_str("\nNo search results found.\n");
        return true;
    }

    if !projects.is_empty() {
        output.push_str("\nProjects\n");
        render_generic_table(output, projects);
    }
    if !contracts.is_empty() {
        output.push_str("\nContracts\n");
        render_search_contracts_table(output, contracts);
    }
    if !assertions.is_empty() {
        output.push_str("\nAssertions\n");
        render_generic_table(output, assertions);
    }
    true
}

fn render_search_contracts_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<32} {:<10} {:<22} Project",
        "Contract", "Network", "Address"
    )
    .expect("write to string");
    for item in items {
        let data = item.get("data").unwrap_or(item);
        let name = data
            .get("contract_name")
            .and_then(Value::as_str)
            .unwrap_or("-");
        let network = data.get("network").and_then(Value::as_str).unwrap_or("-");
        let address = data.get("address").and_then(Value::as_str).unwrap_or("-");
        let project = data
            .get("related_project_slug")
            .or_else(|| data.get("related_project_id"))
            .and_then(Value::as_str)
            .unwrap_or("-");
        writeln!(
            output,
            "{:<32} {:<10} {:<22} {}",
            pad(name, 32),
            pad(network, 10),
            pad(address, 22),
            project
        )
        .expect("write to string");
    }
}

fn render_account_detail(output: &mut String, data: &Value) -> bool {
    if data.get("email").is_none() || data.get("authMethod").is_none() {
        return false;
    }
    output.push_str("\nAccount\n");
    write_string_field(output, "Email", data, "email");
    write_string_field(output, "User ID", data, "id");
    write_string_field(output, "Auth method", data, "authMethod");
    write_string_field(output, "Scope", data, "scope");
    write_bool_field(output, "Whitelisted", data, "whitelisted");
    write_bool_field(output, "Terms accepted", data, "terms_accepted");
    write_timestamp_field(output, "Terms accepted at", data, "terms_accepted_at");
    true
}

fn render_deployment_state(output: &mut String, data: &Value) -> bool {
    let Some(project) = data.get("project") else {
        return false;
    };
    if data.get("available_contracts").is_none()
        || data.get("submitted_assertions").is_none()
        || data.get("staging_assertions").is_none()
    {
        return false;
    }
    output.push_str("\nDeployments\n");
    if let Some(name) = project.get("project_name").and_then(Value::as_str) {
        writeln!(output, "Project: {name}").expect("write to string");
    }
    if let Some(id) = project.get("project_id").and_then(Value::as_str) {
        writeln!(output, "Project ID: {id}").expect("write to string");
    }
    write_network_list_for_value(output, project);
    write_count_field(output, "Available contracts", data, "available_contracts");
    write_count_field(output, "Submitted assertions", data, "submitted_assertions");
    write_count_field(output, "Staging assertions", data, "staging_assertions");
    if let Some(meta) = data.get("_meta") {
        render_collection_meta(output, meta);
    }
    true
}

fn render_transfer_state(output: &mut String, data: &Value) -> bool {
    let (Some(incoming), Some(outgoing)) = (data.get("incoming"), data.get("outgoing")) else {
        return false;
    };
    output.push_str("\nProtocol manager transfers\n");
    write_transfer_counts(output, "Incoming", incoming);
    write_transfer_counts(output, "Outgoing", outgoing);
    true
}

fn render_integration_status(output: &mut String, data: &Value) -> bool {
    if data.get("configured").is_none() || data.get("enabled").is_none() {
        return false;
    }
    output.push_str("\nIntegration\n");
    write_bool_field(output, "Configured", data, "configured");
    write_bool_field(output, "Enabled", data, "enabled");
    write_optional_string_field(output, "Webhook URL", data, "webhook_url");
    write_timestamp_field(output, "Last notification", data, "last_notification_at");
    write_u64_field(
        output,
        "Notifications sent",
        data,
        "notification_count",
        None,
    );
    write_bool_field(output, "Test available", data, "test_available");
    true
}

fn render_protocol_manager_status(output: &mut String, data: &Value) -> bool {
    if data.get("has_pending_transfer").is_none()
        || data.get("contracts_pending").is_none()
        || data.get("contracts_total").is_none()
    {
        return false;
    }
    output.push_str("\nProtocol manager\n");
    write_bool_field(output, "Pending transfer", data, "has_pending_transfer");
    write_optional_string_field(output, "Current manager", data, "current_manager_address");
    write_optional_string_field(output, "New manager", data, "new_manager_address");
    write_u64_field(output, "Contracts pending", data, "contracts_pending", None);
    write_u64_field(output, "Contracts total", data, "contracts_total", None);
    true
}

fn render_mutation_success(output: &mut String, envelope: &Value, data: &Value) -> bool {
    if data.get("success").and_then(Value::as_bool) != Some(true)
        || data
            .as_object()
            .is_some_and(|object| object.contains_key("message"))
    {
        return false;
    }
    let Some(request) = envelope.get("request") else {
        return false;
    };
    let method = request.get("method").and_then(Value::as_str).unwrap_or("");
    let path = request.get("path").and_then(Value::as_str).unwrap_or("");
    output.push('\n');
    output.push_str(mutation_success_message(method, path));
    output.push('\n');
    true
}

fn mutation_success_message(method: &str, path: &str) -> &'static str {
    match (method, path) {
        ("POST", "/projects/saved") => "Project saved",
        ("DELETE", "/projects/saved") => "Project removed from saved projects",
        _ if method == "DELETE"
            && path.starts_with("/projects/")
            && path.contains("/invitations/") =>
        {
            "Invitation revoked"
        }
        _ if method == "POST" && path.starts_with("/projects/") && path.ends_with("/resend") => {
            "Invitation resent"
        }
        _ if method == "PATCH" && path.starts_with("/projects/") && path.contains("/members/") => {
            "Member role updated"
        }
        _ if method == "DELETE" && path.starts_with("/projects/") && path.contains("/members/") => {
            "Member removed"
        }
        _ if method == "DELETE" && path.ends_with("/protocol-manager") => {
            "Protocol manager cleared"
        }
        _ if method == "POST" && path.ends_with("/confirm-transfer") => {
            "Protocol manager transfer confirmed"
        }
        _ if method == "DELETE"
            && path.starts_with("/projects/")
            && !path.contains("/integrations/")
            && !path.contains("/invitations/")
            && !path.contains("/members/")
            && !path.contains("/protocol-manager") =>
        {
            "Project deleted"
        }
        _ => "Request completed",
    }
}

fn render_body_template(output: &mut String, envelope: &Value, data: &Value) -> bool {
    if !is_body_template_envelope(envelope) {
        return false;
    }
    if let Some(variants) = data.get("body_variants").and_then(Value::as_array) {
        output.push_str("\nBody variants\n");
        for variant in variants {
            let name = variant
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("variant");
            writeln!(output, "- {name}").expect("write to string");
            if let Some(body) = variant.get("body") {
                render_human_value(output, body, 4);
            }
        }
        return true;
    }

    let Some(object) = data.as_object() else {
        return false;
    };
    if object.is_empty()
        || !object
            .values()
            .all(|value| is_scalar(value) || value.is_object() || value.is_array())
    {
        return false;
    }
    if !object.keys().any(|key| is_body_template_key(key)) {
        return false;
    }
    output.push_str("\nBody template\n");
    render_human_value(output, data, 2);
    true
}

fn is_body_template_envelope(envelope: &Value) -> bool {
    envelope
        .get("next_actions")
        .and_then(Value::as_array)
        .is_some_and(|actions| {
            actions.iter().filter_map(Value::as_str).any(|action| {
                action.starts_with("Pass the template")
                    || action.starts_with("Choose one entry from data.body_variants")
            })
        })
}

fn render_api_manifest(output: &mut String, data: &Value) -> bool {
    if data.get("name").and_then(Value::as_str) != Some("pcl") || data.get("commands").is_none() {
        return false;
    }
    output.push_str("\nPCL command surface\n");
    if let Some(description) = data.get("description").and_then(Value::as_str) {
        writeln!(output, "{description}").expect("write to string");
    }
    output.push_str("\nStart here:\n");
    for command in ["pcl --llms", "pcl workflows", "pcl schema list"] {
        writeln!(output, "  - {command}").expect("write to string");
    }
    if let Some(commands) = data.get("commands").and_then(Value::as_array) {
        writeln!(
            output,
            "\n{} workflow/API command groups available.",
            commands.len()
        )
        .expect("write to string");
    }
    true
}

fn render_llms_guide(output: &mut String, data: &Value) -> bool {
    if data.get("purpose").is_none() || data.get("consumption_order").is_none() {
        return false;
    }
    output.push_str("\nLLM guide\n");
    if let Some(purpose) = data.get("purpose").and_then(Value::as_str) {
        writeln!(output, "{purpose}").expect("write to string");
    }
    if let Some(order) = data.get("consumption_order").and_then(Value::as_array) {
        output.push_str("\nRecommended order:\n");
        for command in order.iter().filter_map(Value::as_str).take(8) {
            writeln!(output, "  - {command}").expect("write to string");
        }
    }
    true
}

fn render_workflow_detail(output: &mut String, data: &Value) -> bool {
    if data.get("steps").is_none() || data.get("name").is_none() {
        return false;
    }
    output.push('\n');
    if let Some(name) = data.get("name").and_then(Value::as_str) {
        writeln!(output, "Workflow: {name}").expect("write to string");
    }
    if let Some(description) = data.get("description").and_then(Value::as_str) {
        writeln!(output, "{description}").expect("write to string");
    }
    if let Some(steps) = data.get("steps").and_then(Value::as_array) {
        output.push_str("\nSteps:\n");
        for (index, step) in steps.iter().enumerate() {
            let command = step.get("command").and_then(Value::as_str).unwrap_or("-");
            let description = step.get("output").and_then(Value::as_str).unwrap_or("");
            writeln!(
                output,
                "  {}. {}{}",
                index + 1,
                humanize_command(command),
                if description.is_empty() {
                    String::new()
                } else {
                    format!(" -> {description}")
                }
            )
            .expect("write to string");
        }
    }
    true
}

fn render_schema_detail(output: &mut String, data: &Value) -> bool {
    if data.get("workflow").is_none()
        || !(data.get("actions").is_some() || data.get("action").is_some())
    {
        return false;
    }
    output.push('\n');
    if let Some(workflow) = data.get("workflow").and_then(Value::as_str) {
        writeln!(output, "Schema: {workflow}").expect("write to string");
    }
    if let Some(command) = data.get("command").and_then(Value::as_str) {
        writeln!(output, "Command: {}", humanize_command(command)).expect("write to string");
    }
    if let Some(actions) = data.get("actions").and_then(Value::as_array) {
        render_actions_table(output, actions);
    } else if let Some(action) = data.get("action") {
        render_action_detail(output, action);
    }
    true
}

fn render_operation_detail(output: &mut String, data: &Value) -> bool {
    if data.get("operation_id").is_none()
        || data.get("method").is_none()
        || data.get("path").is_none()
    {
        return false;
    }
    output.push_str("\nAPI operation\n");
    let method = data.get("method").and_then(Value::as_str).unwrap_or("-");
    let path = data.get("path").and_then(Value::as_str).unwrap_or("-");
    writeln!(output, "{method} {path}").expect("write to string");
    if let Some(operation_id) = data.get("operation_id").and_then(Value::as_str) {
        writeln!(output, "Operation: {operation_id}").expect("write to string");
    }
    if let Some(summary) = data.get("summary").and_then(Value::as_str) {
        writeln!(output, "Summary: {summary}").expect("write to string");
    }
    if let Some(policy) = data.pointer("/raw_api_use/policy").and_then(Value::as_str) {
        writeln!(output, "Raw API policy: {}", human_label(policy)).expect("write to string");
    }
    if let Some(alternatives) = data.get("workflow_alternatives").and_then(Value::as_array)
        && !alternatives.is_empty()
    {
        output.push_str("Prefer:\n");
        for alternative in alternatives {
            if let Some(example) = alternative.get("example").and_then(Value::as_str) {
                writeln!(output, "  - {}", humanize_command(example)).expect("write to string");
            }
        }
    }
    if let Some(command) = data.get("call_command").and_then(Value::as_str) {
        writeln!(output, "Raw call: {}", humanize_command(command)).expect("write to string");
    }
    true
}

fn render_api_coverage(output: &mut String, data: &Value) -> bool {
    let Some(total) = data.get("total_operations").and_then(Value::as_u64) else {
        return false;
    };
    output.push_str("\nAPI coverage\n");
    writeln!(output, "Operations: {total}").expect("write to string");
    for (label, field) in [
        ("No request-log hit", "no_hit_count"),
        ("Hit without 2xx", "no_2xx_count"),
        ("Write hit without 2xx", "write_no_2xx_count"),
        ("Unmatched records", "unmatched_record_count"),
    ] {
        if let Some(count) = data.get(field).and_then(Value::as_u64) {
            writeln!(output, "{label}: {count}").expect("write to string");
        }
    }
    if let Some(by_method) = data.get("by_method").and_then(Value::as_object) {
        output.push_str("\nBy method:\n");
        for (method, stats) in by_method {
            let total = stats.get("total").and_then(Value::as_u64).unwrap_or(0);
            let hit = stats.get("hit").and_then(Value::as_u64).unwrap_or(0);
            let ok = stats.get("ok").and_then(Value::as_u64).unwrap_or(0);
            writeln!(output, "  {method}: {ok}/{total} 2xx, {hit} hit").expect("write to string");
        }
    }
    true
}

fn render_raw_api_response(output: &mut String, data: &Value) -> bool {
    if data.get("request").is_none() || data.get("response").is_none() {
        return false;
    }
    let request = data.get("request").unwrap_or(&Value::Null);
    let response = data.get("response").unwrap_or(&Value::Null);
    output.push_str("\nAPI response\n");
    if let (Some(method), Some(path)) = (
        request.get("method").and_then(Value::as_str),
        request.get("path").and_then(Value::as_str),
    ) {
        writeln!(output, "{method} {path}").expect("write to string");
    }
    if let Some(status) = response.get("status").and_then(Value::as_u64) {
        writeln!(output, "HTTP {status}").expect("write to string");
    }
    if let Some(request_id) = response.get("request_id").and_then(Value::as_str) {
        writeln!(output, "Request ID: {request_id}").expect("write to string");
    }
    if let Some(body) = response.get("body") {
        if let Some(collection) = find_collection_in_value(body, "") {
            output.push('\n');
            output.push_str(&collection.name);
            output.push('\n');
            output.push_str(&collection_summary(&collection));
            output.push_str("\n\n");
            if collection.items.is_empty() {
                writeln!(output, "No {} found.", collection.name.to_ascii_lowercase())
                    .expect("write to string");
            } else {
                render_collection_items(output, &collection);
            }
        } else {
            output.push_str("Body: ");
            output.push_str(&human_compact_summary(body));
            output.push('\n');
        }
    }
    if let Some(path) = data.get("output_path").and_then(Value::as_str) {
        writeln!(output, "Wrote: {path}").expect("write to string");
    }
    true
}

fn render_export_result(output: &mut String, data: &Value) -> bool {
    if data.get("export").and_then(Value::as_str) != Some("incidents")
        && !(data.get("plan").is_some() && data.get("job_id").is_some())
    {
        return false;
    }
    output.push_str("\nIncident export\n");
    if let Some(job_id) = data.get("job_id").and_then(Value::as_str) {
        writeln!(output, "Job: {job_id}").expect("write to string");
    }
    let source = data.get("plan").unwrap_or(data);
    for (label, field) in [
        ("Output", "out"),
        ("Errors", "errors"),
        ("Checkpoint", "checkpoint"),
    ] {
        if let Some(path) = source.get(field).and_then(Value::as_str) {
            writeln!(output, "{label}: {path}").expect("write to string");
        }
    }
    for (label, field) in [
        ("Pages fetched", "pages_fetched"),
        ("Incidents written", "incidents_written"),
        ("Errors written", "errors_written"),
        ("Retries", "retries_attempted"),
    ] {
        if let Some(count) = data.get(field).and_then(Value::as_u64) {
            writeln!(output, "{label}: {count}").expect("write to string");
        }
    }
    if let Some(command) = data.get("resume_command").and_then(Value::as_str) {
        writeln!(output, "Resume: {}", humanize_command(command)).expect("write to string");
    }
    true
}

fn render_job_detail(output: &mut String, data: &Value) -> bool {
    let job = data.get("job").unwrap_or(data);
    if job.get("job_id").is_none() {
        return false;
    }
    output.push_str("\nJob\n");
    for (label, field) in [
        ("ID", "job_id"),
        ("Kind", "kind"),
        ("Status", "status"),
        ("Updated", "updated_at"),
    ] {
        if let Some(value) = job.get(field) {
            writeln!(output, "{label}: {}", human_cell(value)).expect("write to string");
        }
    }
    if let Some(stats) = job.get("stats") {
        output.push_str("Stats: ");
        output.push_str(&human_compact_summary(stats));
        output.push('\n');
    }
    if let Some(command) = data
        .get("resume_command")
        .or_else(|| job.get("resume_command"))
        .and_then(Value::as_str)
    {
        writeln!(output, "Resume: {}", humanize_command(command)).expect("write to string");
    }
    true
}

fn render_path_or_toggle_result(output: &mut String, data: &Value) -> bool {
    if data
        .as_object()
        .is_some_and(|object| object.values().any(Value::is_array))
    {
        return false;
    }
    let path_fields = [
        ("Config", "config_path"),
        ("Artifacts", "artifact_dir"),
        ("Request log", "request_log"),
        ("Jobs", "jobs_path"),
    ];
    let mut rendered = false;
    for (label, field) in path_fields {
        if let Some(path) = data.get(field).and_then(Value::as_str) {
            if !rendered {
                output.push('\n');
                rendered = true;
            }
            writeln!(output, "{label}: {path}").expect("write to string");
        }
    }
    for (label, field) in [("Created", "created"), ("Deleted", "deleted")] {
        if let Some(value) = data.get(field).and_then(Value::as_bool) {
            if !rendered {
                output.push('\n');
                rendered = true;
            }
            writeln!(output, "{label}: {}", yes_no(value)).expect("write to string");
        }
    }
    rendered
}

fn write_string_field(output: &mut String, label: &str, data: &Value, field: &str) {
    if let Some(value) = data.get(field).and_then(Value::as_str) {
        writeln!(output, "{label}: {value}").expect("write to string");
    }
}

fn write_optional_string_field(output: &mut String, label: &str, data: &Value, field: &str) {
    match data.get(field) {
        Some(Value::String(value)) if !value.is_empty() => {
            writeln!(output, "{label}: {value}").expect("write to string");
        }
        Some(Value::Null) | None => {}
        Some(value) if is_scalar(value) => {
            writeln!(output, "{label}: {}", scalar_string(value)).expect("write to string");
        }
        Some(_) => {}
    }
}

fn write_timestamp_field(output: &mut String, label: &str, data: &Value, field: &str) {
    if let Some(value) = data.get(field).and_then(Value::as_str) {
        writeln!(output, "{label}: {}", format_timestamp(value)).expect("write to string");
    }
}

fn write_bool_field(output: &mut String, label: &str, data: &Value, field: &str) {
    if let Some(value) = data.get(field).and_then(Value::as_bool) {
        writeln!(output, "{label}: {}", yes_no(value)).expect("write to string");
    }
}

fn write_u64_field(
    output: &mut String,
    label: &str,
    data: &Value,
    field: &str,
    unit: Option<&str>,
) {
    if let Some(value) = data.get(field).and_then(Value::as_u64) {
        if let Some(unit) = unit {
            writeln!(output, "{label}: {value} {unit}").expect("write to string");
        } else {
            writeln!(output, "{label}: {value}").expect("write to string");
        }
    }
}

fn write_count_field(output: &mut String, label: &str, data: &Value, field: &str) {
    if let Some(values) = data.get(field).and_then(Value::as_array) {
        writeln!(
            output,
            "{label}: {}",
            plural_count(values.len(), count_field_unit(label, field))
        )
        .expect("write to string");
    }
}

fn count_field_unit(label: &str, field: &str) -> &'static str {
    match (label, field) {
        ("Available contracts", _) => "contract",
        ("Submitted assertions", _) | ("Staging assertions", _) => "assertion",
        (_, "available_contracts") => "contract",
        (_, "submitted_assertions" | "staging_assertions" | "submitted_assertion_ids") => {
            "assertion"
        }
        _ => "item",
    }
}

fn write_network_list_for_value(output: &mut String, data: &Value) {
    let names = data
        .get("chain_names")
        .or_else(|| data.get("project_networks"))
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(ToString::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if names.is_empty() {
        return;
    }
    writeln!(output, "Networks: {}", names.join(", ")).expect("write to string");
}

fn write_transfer_counts(output: &mut String, label: &str, value: &Value) {
    let projects = value
        .get("project_transfers")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let contracts = value
        .get("contract_transfers")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    writeln!(
        output,
        "{label}: {}, {}",
        plural_count(projects, "project transfer"),
        plural_count(contracts, "contract transfer")
    )
    .expect("write to string");
}

fn render_human_collection(output: &mut String, envelope: &Value) -> bool {
    let Some(collection) = find_human_collection(envelope) else {
        return false;
    };

    output.push('\n');
    output.push_str(&collection.name);
    output.push('\n');
    output.push_str(&collection_summary(&collection));
    output.push('\n');
    if let Some(meta) = collection.meta {
        render_collection_meta(output, meta);
    }
    output.push('\n');

    if collection.items.is_empty() {
        writeln!(output, "No {} found.", collection.name.to_ascii_lowercase())
            .expect("write to string");
        return true;
    }

    render_collection_items(output, &collection);

    if let Some(pagination) = collection.pagination
        && pagination
            .get("hasMore")
            .or_else(|| pagination.get("has_more"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        let next_page = pagination
            .get("page")
            .and_then(Value::as_u64)
            .map_or(2, |page| page.saturating_add(1));
        let limit = pagination
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(collection.items.len() as u64);
        output.push('\n');
        writeln!(
            output,
            "More results available. Try --page {next_page} --limit {limit}."
        )
        .expect("write to string");
    }

    true
}

fn find_human_collection(envelope: &Value) -> Option<HumanCollection<'_>> {
    let data = envelope.get("data")?;
    let request_path = envelope
        .pointer("/request/path")
        .and_then(Value::as_str)
        .unwrap_or_default();

    find_collection_in_value(data, request_path)
}

fn find_collection_in_value<'a>(
    data: &'a Value,
    request_path: &str,
) -> Option<HumanCollection<'a>> {
    if let Some(inner) = data.get("data")
        && let Some(collection) = find_collection_in_value(inner, request_path)
    {
        return Some(HumanCollection {
            meta: data.get("_meta").or(collection.meta),
            ..collection
        });
    }

    if let Some(items) = data.as_array() {
        return Some(HumanCollection {
            field: infer_collection_field(request_path),
            name: infer_collection_name("items", request_path, items),
            items,
            pagination: None,
            meta: None,
        });
    }

    if let Some(items) = data.get("items").and_then(Value::as_array) {
        return Some(HumanCollection {
            field: "items".to_string(),
            name: infer_collection_name("items", request_path, items),
            items,
            pagination: data.get("pagination"),
            meta: data.get("_meta"),
        });
    }

    for field in [
        "incidents",
        "assertions",
        "contracts",
        "releases",
        "projects",
        "deployments",
        "events",
        "operations",
        "workflows",
        "schemas",
        "checks",
        "records",
        "jobs",
        "artifacts",
        "members",
        "invitations",
        "integrations",
        "transfers",
        "requests",
        "no_hit",
        "no_2xx",
        "write_no_2xx",
        "unmatched_records",
        "body_variants",
        "examples",
        "product_surfaces",
    ] {
        if let Some(items) = data.get(field).and_then(Value::as_array) {
            return Some(HumanCollection {
                field: field.to_string(),
                name: human_label(field),
                items,
                pagination: data.get("pagination"),
                meta: data.get("_meta"),
            });
        }
    }

    None
}

fn infer_collection_field(request_path: &str) -> String {
    if request_path.contains("assertion_adopters") {
        return "contracts".to_string();
    }
    for field in [
        "incidents",
        "projects",
        "assertions",
        "contracts",
        "releases",
        "deployments",
        "events",
        "members",
        "invitations",
        "transfers",
    ] {
        if request_path.contains(field) {
            return field.to_string();
        }
    }
    "items".to_string()
}

fn infer_collection_name(field: &str, request_path: &str, items: &[Value]) -> String {
    if request_path.contains("assertion_adopters") {
        return "Contracts".to_string();
    }
    for name in [
        "incidents",
        "assertions",
        "contracts",
        "releases",
        "projects",
        "deployments",
        "events",
        "operations",
        "workflows",
        "schemas",
        "records",
        "jobs",
        "artifacts",
        "requests",
    ] {
        if request_path.contains(name) {
            return human_label(name);
        }
    }
    if items.iter().any(has_incident_shape) {
        return "Incidents".to_string();
    }
    human_label(field)
}

fn collection_summary(collection: &HumanCollection<'_>) -> String {
    let shown = collection.items.len();
    if let Some(pagination) = collection.pagination {
        let total = pagination
            .get("total")
            .and_then(Value::as_u64)
            .unwrap_or(shown as u64);
        let page = pagination.get("page").and_then(Value::as_u64);
        let limit = pagination.get("limit").and_then(Value::as_u64);
        let item_name = collection_item_name(&collection.name, total);
        let mut summary = if total > shown as u64 {
            format!("Showing {shown} of {total} {item_name}")
        } else {
            format!("Showing {shown} {item_name}")
        };
        if let Some(page) = page {
            write!(summary, " on page {page}").expect("write to string");
        }
        if let Some(limit) = limit {
            write!(summary, " (limit {limit})").expect("write to string");
        }
        return summary;
    }
    let item_name = collection_item_name(&collection.name, shown as u64);
    format!("Showing {shown} {item_name}")
}

fn collection_item_name(name: &str, count: u64) -> String {
    let lower = name.to_ascii_lowercase();
    if count != 1 {
        return lower;
    }
    lower.strip_suffix("ies").map_or_else(
        || lower.strip_suffix("s").unwrap_or(&lower).to_string(),
        |stem| format!("{stem}y"),
    )
}

fn render_collection_items(output: &mut String, collection: &HumanCollection<'_>) {
    match collection.field.as_str() {
        "checks" => render_checks_table(output, collection.items),
        "operations" => render_operations_table(output, collection.items),
        "workflows" => render_workflows_table(output, collection.items),
        "schemas" => render_schemas_table(output, collection.items),
        "records" | "requests" | "unmatched_records" => {
            render_request_records_table(output, collection.items);
        }
        "jobs" => render_jobs_table(output, collection.items),
        "artifacts" => render_artifacts_table(output, collection.items),
        "members" => render_members_table(output, collection.items),
        "invitations" => render_invitations_table(output, collection.items),
        "projects" => render_projects_table(output, collection.items),
        "releases" => render_releases_table(output, collection.items),
        "events" => render_events_table(output, collection.items),
        "no_hit" | "no_2xx" | "write_no_2xx" => render_coverage_table(output, collection.items),
        "body_variants" => render_body_variant_table(output, collection.items),
        _ if is_incident_collection(collection) => render_incident_table(output, collection.items),
        _ => render_generic_table(output, collection.items),
    }
}

macro_rules! render_rows {
    ($output:expr, $items:expr, $header:expr, $row:literal, |$item:ident| $($arg:expr),+ $(,)?) => {{
        writeln!($output, "{}", $header).expect("write to string");
        for $item in $items {
            writeln!($output, $row, $($arg),+).expect("write to string");
        }
    }};
}

fn str_field<'a>(item: &'a Value, field: &str) -> &'a str {
    item.get(field).and_then(Value::as_str).unwrap_or("-")
}

fn str_any<'a>(item: &'a Value, fields: &[&str], default: &'static str) -> &'a str {
    fields
        .iter()
        .find_map(|field| item.get(*field).and_then(Value::as_str))
        .unwrap_or(default)
}

fn render_checks_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!("{:<20} {:<10} Details", "Check", "Status"),
        "{:<20} {:<10} {}",
        |item| pad(str_field(item, "name"), 20),
        pad(str_field(item, "status"), 10),
        item.get("details")
            .or_else(|| item.get("path"))
            .map_or_else(String::new, human_compact_summary),
    );
}

fn render_operations_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!("{:<7} {:<45} {:<36} Policy", "Method", "Path", "Operation"),
        "{:<7} {:<45} {:<36} {}",
        |item| str_field(item, "method"),
        pad(str_field(item, "path"), 45),
        pad(str_field(item, "operation_id"), 36),
        human_label(
            item.pointer("/raw_api_use/policy")
                .and_then(Value::as_str)
                .unwrap_or("-"),
        ),
    );
}

fn render_workflows_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!("{:<28} Steps  Description", "Workflow"),
        "{:<28} {:<5} {}",
        |item| pad(str_field(item, "name"), 28),
        item.get("steps")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        truncate(
            item.get("description")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            72,
        ),
    );
}

fn render_schemas_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!("{:<24} {:<7} Command", "Workflow", "Actions"),
        "{:<24} {:<7} {}",
        |item| pad(str_field(item, "workflow"), 24),
        item.get("actions").and_then(Value::as_u64).unwrap_or(0),
        truncate(&humanize_command(str_field(item, "command")), 96),
    );
}

fn render_request_records_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!(
            "{:<16} {:<7} {:<45} {:<6} Request ID",
            "Time", "Method", "Path", "HTTP"
        ),
        "{:<16} {:<7} {:<45} {:<6} {}",
        |item| {
            pad(
                &item
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .map_or_else(String::new, format_timestamp),
                16,
            )
        },
        str_field(item, "method"),
        pad(str_field(item, "path"), 45),
        item.get("status")
            .and_then(Value::as_u64)
            .map_or_else(|| "-".to_string(), |value| value.to_string()),
        str_field(item, "request_id"),
    );
}

fn render_jobs_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!("{:<38} {:<16} {:<12} Updated", "Job", "Kind", "Status"),
        "{:<38} {:<16} {:<12} {}",
        |item| pad(str_field(item, "job_id"), 38),
        pad(str_field(item, "kind"), 16),
        pad(str_field(item, "status"), 12),
        item.get("updated_at")
            .and_then(Value::as_str)
            .map_or_else(String::new, format_timestamp),
    );
}

fn render_artifacts_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!("{:<58} {:>10} Modified", "Path", "Bytes"),
        "{:<58} {:>10} {}",
        |item| pad(str_field(item, "path"), 58),
        item.get("bytes")
            .and_then(Value::as_u64)
            .map_or_else(|| "-".to_string(), |value| value.to_string()),
        item.get("modified")
            .and_then(Value::as_u64)
            .map_or_else(String::new, format_unix_timestamp),
    );
}

fn render_projects_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!(
            "{:<28} {:<22} {:<20} {:<10} ID",
            "Project", "Slug", "Network", "Visibility"
        ),
        "{:<28} {:<22} {:<20} {:<10} {}",
        |item| pad(str_any(item, &["project_name", "name"], "-"), 28),
        pad(str_field(item, "slug"), 22),
        pad(&first_project_network(item), 20),
        item.get("is_private")
            .and_then(Value::as_bool)
            .map_or("-", |private| if private { "private" } else { "public" }),
        str_any(item, &["project_id", "id"], "-"),
    );
}

fn first_project_network(item: &Value) -> String {
    item.get("chain_names")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
        .or_else(|| {
            item.get("project_networks")
                .and_then(Value::as_array)
                .and_then(|values| values.first())
        })
        .map_or_else(|| "-".to_string(), human_scalar)
}

fn render_members_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!("{:<34} {:<12} User ID", "Email", "Role"),
        "{:<34} {:<12} {}",
        |item| pad(str_field(item, "email"), 34),
        pad(str_field(item, "role"), 12),
        str_field(item, "user_id"),
    );
}

fn render_invitations_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!("{:<34} {:<12} {:<16} ID", "Email", "Role", "Status"),
        "{:<34} {:<12} {:<16} {}",
        |item| {
            pad(
                str_any(item, &["email", "identifier", "invitee_identifier"], "-"),
                34,
            )
        },
        pad(str_field(item, "role"), 12),
        pad(str_any(item, &["status"], "pending"), 16),
        str_any(item, &["id", "invitation_id"], "-"),
    );
}

fn render_releases_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!(
            "{:<36} {:<14} {:<16} Created",
            "Release", "Environment", "Status"
        ),
        "{:<36} {:<14} {:<16} {}",
        |item| pad(str_any(item, &["release_id", "id"], "-"), 36),
        pad(str_field(item, "environment"), 14),
        pad(str_field(item, "status"), 16),
        item.get("created_at")
            .or_else(|| item.get("createdAt"))
            .and_then(Value::as_str)
            .map_or_else(String::new, format_timestamp),
    );
}

fn render_events_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items,
        format!("{:<34} {:<14} {:<16} Type", "Event", "Environment", "Time"),
        "{:<34} {:<14} {:<16} {}",
        |item| pad(str_field(item, "id"), 34),
        pad(str_field(item, "environment"), 14),
        pad(
            &item
                .get("timestamp")
                .or_else(|| item.get("created_at"))
                .and_then(Value::as_str)
                .map_or_else(String::new, format_timestamp),
            16,
        ),
        str_any(item, &["type", "event_type"], "-"),
    );
}

fn render_coverage_table(output: &mut String, items: &[Value]) {
    render_rows!(
        output,
        items.iter().take(20),
        format!(
            "{:<7} {:<45} {:<7} {:<7} Request ID",
            "Method", "Path", "Hits", "2xx"
        ),
        "{:<7} {:<45} {:<7} {:<7} {}",
        |item| str_field(item, "method"),
        pad(str_field(item, "path"), 45),
        item.get("hits").and_then(Value::as_u64).unwrap_or(0),
        item.get("ok").and_then(Value::as_u64).unwrap_or(0),
        str_field(item, "latest_request_id"),
    );
    if items.len() > 20 {
        writeln!(output, "... {} more", items.len() - 20).expect("write to string");
    }
}

fn render_body_variant_table(output: &mut String, items: &[Value]) {
    for item in items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("variant");
        writeln!(output, "- {name}").expect("write to string");
        if let Some(body) = item.get("body") {
            render_human_value(output, body, 4);
        }
    }
}

fn render_collection_meta(output: &mut String, meta: &Value) {
    let fetched_at = meta
        .get("fetchedAt")
        .or_else(|| meta.get("fetched_at"))
        .and_then(Value::as_str);
    let sources = meta.get("sources").and_then(Value::as_array);
    if fetched_at.is_none() && sources.is_none_or(Vec::is_empty) {
        return;
    }

    if let Some(fetched_at) = fetched_at {
        output.push_str("Updated: ");
        output.push_str(&format_timestamp(fetched_at));
        output.push('\n');
    }
    if let Some(sources) = sources {
        let source_names = sources
            .iter()
            .filter_map(Value::as_str)
            .map(human_source_name)
            .collect::<Vec<_>>()
            .join(", ");
        if !source_names.is_empty() {
            output.push_str("Source: ");
            output.push_str(&source_names);
            output.push('\n');
        }
    }
}

fn human_source_name(source: &str) -> String {
    match source {
        "offchain" => "Phylax platform index".to_string(),
        "onchain" => "on-chain data".to_string(),
        "cache" => "cache".to_string(),
        other => human_label(other),
    }
}

fn is_incident_collection(collection: &HumanCollection<'_>) -> bool {
    collection.name == "Incidents" || collection.items.iter().any(has_incident_shape)
}

fn has_incident_shape(value: &Value) -> bool {
    value.get("referenceId").is_some()
        || value.get("reference_id").is_some()
        || (value.get("timestamp").is_some()
            && value.get("network").is_some()
            && value.get("title").is_some())
}

fn render_incident_table(output: &mut String, items: &[Value]) {
    writeln!(
        output,
        "{:<3} {:<16} {:<24} {:<29} ID",
        "#", "Time", "Network", "Title"
    )
    .expect("write to string");
    for (index, item) in items.iter().enumerate() {
        let timestamp = item
            .get("timestamp")
            .and_then(Value::as_str)
            .map_or_else(String::new, format_timestamp);
        let network = format_network(item.get("network"));
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled");
        let id = item.get("id").and_then(Value::as_str).unwrap_or("-");
        writeln!(
            output,
            "{:<3} {:<16} {:<24} {:<29} {}",
            index + 1,
            pad(&timestamp, 16),
            pad(&network, 24),
            pad(title, 29),
            id
        )
        .expect("write to string");
    }
}

fn render_generic_table(output: &mut String, items: &[Value]) {
    let columns = generic_columns(items);
    if columns.is_empty() {
        render_human_value(output, &Value::Array(items.to_vec()), 0);
        return;
    }

    write!(output, "{:<3}", "#").expect("write to string");
    for column in &columns {
        write!(output, " {:<22}", human_label(column)).expect("write to string");
    }
    output.push('\n');

    for (index, item) in items.iter().enumerate() {
        write!(output, "{:<3}", index + 1).expect("write to string");
        for column in &columns {
            let value = item.get(column).map_or_else(String::new, human_cell);
            write!(output, " {:<22}", pad(&value, 22)).expect("write to string");
        }
        output.push('\n');
    }
}

fn generic_columns(items: &[Value]) -> Vec<String> {
    let mut columns = Vec::new();
    for preferred in [
        "name",
        "title",
        "id",
        "status",
        "environment",
        "network",
        "timestamp",
        "createdAt",
        "updatedAt",
    ] {
        if items.iter().any(|item| item.get(preferred).is_some()) {
            columns.push(preferred.to_string());
        }
        if columns.len() == 4 {
            return columns;
        }
    }

    if columns.is_empty()
        && let Some(object) = items.first().and_then(Value::as_object)
    {
        columns.extend(object.keys().take(4).cloned());
    }
    columns
}

fn human_cell(value: &Value) -> String {
    match value {
        Value::Object(object) if object.contains_key("name") => {
            object
                .get("name")
                .and_then(Value::as_str)
                .map_or_else(|| compact_json(value), ToString::to_string)
        }
        Value::Object(_) | Value::Array(_) => compact_json(value),
        _ => human_scalar(value),
    }
}

fn human_action_str(value: &str) -> String {
    if value.trim_start().starts_with("pcl ") {
        humanize_command(value)
    } else if matches!(
        value,
        "Use --toon for agent consumption or --json for strict JSON parsing"
            | "Use --json for strict JSON parsing"
    ) {
        String::new()
    } else if value == "Use --body-template when constructing mutation bodies" {
        "Use --body-template to start from an example request body".to_string()
    } else {
        value.to_string()
    }
}

fn humanize_command(command: &str) -> String {
    command
        .replace(" --format toon", "")
        .replace(" --toon", "")
        .replace(" --json", "")
        .replace("--toon ", "")
        .replace("--json ", "")
}

fn is_body_template_key(key: &str) -> bool {
    matches!(
        key,
        "project_name"
            | "project_description"
            | "profile_image_url"
            | "github_url"
            | "chain_id"
            | "is_private"
            | "is_dev"
            | "project_id"
            | "identifier"
            | "identifier_type"
            | "role"
            | "provider"
            | "webhook_url"
            | "routing_key"
            | "enabled"
            | "address"
            | "signature"
            | "nonce"
            | "tx_hash"
            | "contract_name"
            | "assertions"
            | "assertionsDir"
            | "contracts"
            | "environment"
            | "mode"
            | "new_manager_address"
            | "ponder_transfer_id"
            | "reason"
            | "notify"
    )
}

fn name_value_pairs(values: &[Value]) -> String {
    values
        .iter()
        .map(|value| {
            let name = value.get("name").and_then(Value::as_str).unwrap_or("?");
            let rendered = value
                .get("value")
                .map_or_else(|| "none".to_string(), scalar_string);
            format!("{name}={rendered}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_actions_table(output: &mut String, actions: &[Value]) {
    writeln!(
        output,
        "{:<24} {:<7} {:<8} Path",
        "Action", "Auth", "Method"
    )
    .expect("write to string");
    for action in actions {
        let name = action.get("name").and_then(Value::as_str).unwrap_or("-");
        let auth = action
            .get("auth")
            .and_then(Value::as_bool)
            .map_or("-", |value| if value { "yes" } else { "no" });
        let method = action.get("method").and_then(Value::as_str).unwrap_or("-");
        let path = action.get("path").and_then(Value::as_str).unwrap_or("-");
        writeln!(
            output,
            "{:<24} {:<7} {:<8} {}",
            pad(name, 24),
            auth,
            method,
            path
        )
        .expect("write to string");
    }
}

fn render_action_detail(output: &mut String, action: &Value) {
    let name = action.get("name").and_then(Value::as_str).unwrap_or("-");
    writeln!(output, "Action: {name}").expect("write to string");
    if let (Some(method), Some(path)) = (
        action.get("method").and_then(Value::as_str),
        action.get("path").and_then(Value::as_str),
    ) {
        writeln!(output, "Request: {method} {path}").expect("write to string");
    }
    if let Some(auth) = action.get("auth").and_then(Value::as_bool) {
        writeln!(
            output,
            "Auth: {}",
            if auth { "required" } else { "not required" }
        )
        .expect("write to string");
    }
    if let Some(example) = action.get("example").and_then(Value::as_str) {
        writeln!(output, "Example: {}", humanize_command(example)).expect("write to string");
    }
    if let Some(flags) = action.get("required_flags").and_then(Value::as_array)
        && !flags.is_empty()
    {
        writeln!(output, "Required flags: {}", string_list(flags)).expect("write to string");
    }
    if let Some(flags) = action.get("optional_flags").and_then(Value::as_array)
        && !flags.is_empty()
    {
        writeln!(output, "Optional flags: {}", string_list(flags)).expect("write to string");
    }
}

fn string_list(values: &[Value]) -> String {
    values
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn format_duration(seconds: i64) -> String {
    if seconds < 0 {
        return "expired".to_string();
    }
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn render_human_summary(output: &mut String, data: &Value) {
    let display_data = data.get("data").unwrap_or(data);
    output.push('\n');
    if let Some(object) = display_data.as_object() {
        for (key, value) in object {
            if key.starts_with('_') {
                continue;
            }
            output.push_str(&human_label(key));
            output.push_str(": ");
            if is_scalar(value) {
                output.push_str(&human_scalar(value));
                output.push('\n');
            } else {
                output.push_str(&human_compact_summary(value));
                output.push('\n');
            }
        }
    } else {
        render_human_value(output, display_data, 0);
    }
}

fn render_human_request_id(output: &mut String, envelope: &Value) {
    let request_id = envelope
        .pointer("/response/request_id")
        .and_then(Value::as_str);
    let status = envelope.pointer("/response/status").and_then(Value::as_u64);
    if request_id.is_none() && status.is_none() {
        return;
    }

    output.push('\n');
    if let Some(request_id) = request_id {
        output.push_str("Request ID: ");
        output.push_str(request_id);
        if let Some(status) = status {
            write!(output, " (HTTP {status})").expect("write to string");
        }
        output.push('\n');
    } else if let Some(status) = status {
        writeln!(output, "HTTP status: {status}").expect("write to string");
    }
}

fn human_compact_summary(value: &Value) -> String {
    match value {
        Value::Array(values) => plural_count(values.len(), "item"),
        Value::Object(object) => {
            if object.is_empty() {
                return "empty object".to_string();
            }
            object
                .iter()
                .filter(|(key, _)| !key.starts_with('_'))
                .take(3)
                .map(|(key, value)| {
                    if is_scalar(value) {
                        format!("{}={}", human_label(key), human_scalar(value))
                    } else {
                        format!("{}={}", human_label(key), compact_json(value))
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
        _ => human_scalar(value),
    }
}

fn format_network(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "-".to_string();
    };
    if let Some(name) = value.as_str() {
        return name.to_string();
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("Unknown network");
    if let Some(chain_id) = value.get("chainId").and_then(Value::as_u64) {
        return format!("{name} ({chain_id})");
    }
    if let Some(chain_id) = value.get("chain_id").and_then(Value::as_u64) {
        return format!("{name} ({chain_id})");
    }
    name.to_string()
}

fn format_timestamp(value: &str) -> String {
    if value.len() >= 16 && value.as_bytes().get(10) == Some(&b'T') {
        return value[..16].replace('T', " ");
    }
    value.to_string()
}

fn format_unix_timestamp(value: u64) -> String {
    let Ok(seconds) = i64::try_from(value) else {
        return value.to_string();
    };
    chrono::DateTime::from_timestamp(seconds, 0).map_or_else(
        || value.to_string(),
        |timestamp| timestamp.format("%Y-%m-%d %H:%M").to_string(),
    )
}

fn human_label(value: &str) -> String {
    let words = split_label_words(value);
    let mut rendered = Vec::new();
    for (index, word) in words.iter().enumerate() {
        let lower = word.to_ascii_lowercase();
        let text = match lower.as_str() {
            "id" => "ID".to_string(),
            "api" => "API".to_string(),
            "http" => "HTTP".to_string(),
            "url" => "URL".to_string(),
            "json" => "JSON".to_string(),
            "cli" => "CLI".to_string(),
            "pcl" => "PCL".to_string(),
            "uuid" => "UUID".to_string(),
            "tx" => "tx".to_string(),
            "github" => "GitHub".to_string(),
            "authmethod" => "auth method".to_string(),
            other if index == 0 => capitalize(other),
            other => other.to_string(),
        };
        rendered.push(text);
    }
    rendered.join(" ")
}

fn split_label_words(value: &str) -> Vec<String> {
    let normalized = value.replace(['_', '-'], " ");
    let mut words = Vec::new();
    for raw in normalized.split_whitespace() {
        let mut current = String::new();
        let chars = raw.chars().collect::<Vec<_>>();
        for (index, ch) in chars.iter().enumerate() {
            if index > 0
                && ch.is_uppercase()
                && chars
                    .get(index.saturating_sub(1))
                    .is_some_and(|previous| previous.is_lowercase() || previous.is_ascii_digit())
            {
                words.push(current);
                current = String::new();
            }
            current.push(*ch);
        }
        if !current.is_empty() {
            words.push(current);
        }
    }
    words
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

fn plural_count(count: impl std::fmt::Display, item: &str) -> String {
    let count = count.to_string();
    if count == "1" {
        format!("1 {item}")
    } else {
        format!("{count} {item}s")
    }
}

fn human_scalar(value: &Value) -> String {
    match value {
        Value::Bool(value) => yes_no(*value).to_string(),
        Value::String(value) => {
            if value.len() >= 16 && value.as_bytes().get(10) == Some(&b'T') {
                format_timestamp(value)
            } else {
                value.clone()
            }
        }
        _ => scalar_string(value),
    }
}

fn pad(value: &str, width: usize) -> String {
    let value = truncate(value, width);
    format!("{value:<width$}")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }
    if max_chars <= 3 {
        return value.chars().take(max_chars).collect();
    }
    let prefix: String = value.chars().take(max_chars - 3).collect();
    format!("{prefix}...")
}

fn is_hex_blob(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("0x") else {
        return false;
    };
    hex.len() > 64 && hex.chars().all(|character| character.is_ascii_hexdigit())
}

fn render_human_value(output: &mut String, value: &Value, indent: usize) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                write_indent(output, indent);
                output.push_str(key);
                output.push_str(": ");
                if is_scalar(value) {
                    output.push_str(&scalar_string(value));
                    output.push('\n');
                } else {
                    output.push('\n');
                    render_human_value(output, value, indent + 2);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                write_indent(output, indent);
                output.push_str("- ");
                if is_scalar(value) {
                    output.push_str(&scalar_string(value));
                    output.push('\n');
                } else {
                    output.push('\n');
                    render_human_value(output, value, indent + 2);
                }
            }
        }
        _ => {
            write_indent(output, indent);
            output.push_str(&scalar_string(value));
            output.push('\n');
        }
    }
}

fn write_indent(output: &mut String, indent: usize) {
    for _ in 0..indent {
        output.push(' ');
    }
}

fn is_scalar(value: &Value) -> bool {
    matches!(
        value,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_)
    )
}

fn scalar_string(value: &Value) -> String {
    match value {
        Value::Null => "none".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(_) | Value::Object(_) => compact_json(value),
    }
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// Render a JSON value as the CLI's compact TOON-style text output.
pub fn toon_string(value: &Value) -> String {
    let mut output = toon_format::encode_default(value).unwrap_or_else(|_| {
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    });
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}
