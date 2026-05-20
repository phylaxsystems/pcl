use super::{
    super::{
        ApiCommandError,
        HttpMethod,
        IncidentsArgs,
        WorkflowRequest,
        definitions::{
            WorkflowActionDefinition,
            WorkflowDefinition,
            WorkflowOutputPolicy,
        },
    },
    first_string_field,
    push_query,
    required_arg,
};
use serde_json::Value;

pub(in crate::api) fn incidents_request(
    args: &IncidentsArgs,
) -> Result<WorkflowRequest, ApiCommandError> {
    if args.all && (args.incident_id.is_some() || args.stats || args.retry_trace) {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--all is only supported for incident list workflows".to_string(),
        });
    }
    if args.stats && args.project_id.is_none() {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--stats requires --project-id".to_string(),
        });
    }
    if args.tx_id.is_some() && args.incident_id.is_none() {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--tx-id requires --incident-id".to_string(),
        });
    }
    if args.retry_trace && args.tx_id.is_none() {
        return Err(ApiCommandError::InvalidWorkflow {
            message: "--retry-trace requires --incident-id and --tx-id".to_string(),
        });
    }

    let mut query = Vec::new();
    push_query(&mut query, "page", args.page);
    push_query(&mut query, "limit", args.limit);

    if let Some(incident_id) = &args.incident_id {
        if args.retry_trace {
            let tx_id = required_arg(args.tx_id.as_deref(), "--tx-id")?;
            return Ok(WorkflowRequest {
                method: HttpMethod::Post,
                path: format!("/incidents/{incident_id}/transactions/{tx_id}/trace/retry"),
                query,
                body: Some("{}".to_string()),
                require_auth: true,
                attach_auth: true,
                next_actions: vec![format!(
                    "pcl incidents --incident-id {incident_id} --tx-id {tx_id}"
                )],
            });
        }
        let path = if let Some(tx_id) = &args.tx_id {
            format!("/views/incidents/{incident_id}/transactions/{tx_id}/trace")
        } else {
            format!("/views/incidents/{incident_id}")
        };
        let next_actions = vec![
            "pcl incidents --limit 5".to_string(),
            format!("pcl api inspect get {}", path),
        ];
        return Ok(WorkflowRequest::get_with_query(
            path,
            query,
            true,
            next_actions,
        ));
    }

    if let Some(project_id) = &args.project_id {
        if args.stats {
            let path = format!("/projects/{project_id}/incidents/stats");
            return Ok(WorkflowRequest::get_with_query(
                path,
                query,
                true,
                vec![format!(
                    "pcl incidents --project-id {project_id} --limit 10"
                )],
            ));
        }
        push_query(&mut query, "assertionId", args.assertion_id.as_deref());
        push_query(
            &mut query,
            "assertionAdopterId",
            args.assertion_adopter_id.as_deref(),
        );
        push_query(&mut query, "environment", args.environment.as_deref());
        push_query(&mut query, "fromDate", args.from_date.as_deref());
        push_query(&mut query, "toDate", args.to_date.as_deref());
        let path = format!("/views/projects/{project_id}/incidents");
        return Ok(WorkflowRequest::get_with_query(
            path,
            query,
            true,
            vec![
                format!("pcl assertions --project-id {project_id}"),
                "pcl incidents --limit 5".to_string(),
            ],
        ));
    }

    push_query(&mut query, "network", args.network);
    push_query(&mut query, "sort", args.sort.as_deref());
    push_query(&mut query, "devMode", args.dev_mode.as_deref());
    Ok(WorkflowRequest::get_with_query(
        "/views/public/incidents",
        query,
        false,
        vec![
            "pcl incidents --project-id <project-id> --limit 10".to_string(),
            "pcl projects list --limit 10".to_string(),
        ],
    ))
}

pub(in crate::api) fn incidents_next_actions(
    data: &Value,
    args: &IncidentsArgs,
    fallback: Vec<String>,
) -> Vec<String> {
    if let Some(incident_id) = &args.incident_id {
        if args.tx_id.is_none()
            && let Some(tx_id) = data
                .get("data")
                .and_then(|data| data.get("invalidating_transactions"))
                .and_then(Value::as_array)
                .and_then(|transactions| transactions.first())
                .and_then(|transaction| {
                    first_string_field(transaction, &["transaction_hash", "id", "tx_id"])
                })
        {
            return vec![
                format!("pcl incidents --incident-id {incident_id} --tx-id {tx_id}"),
                "pcl incidents --limit 5".to_string(),
            ];
        }
        return fallback;
    }
    first_string_field(data, &["id", "incidentId", "incident_id"]).map_or(fallback, |incident_id| {
        vec![
            format!("pcl incidents --incident-id {incident_id}"),
            "pcl projects list --limit 10".to_string(),
        ]
    })
}

pub(in crate::api) const DEFINITION: WorkflowDefinition = WorkflowDefinition {
    name: "incidents",
    command: "pcl incidents [--project-id <id>] [--incident-id <id>] [--stats] [--limit <n>] [--all --output <file>]",
    description: "List public incidents, project incidents, fetch all incident pages, inspect incident detail, incident stats, or incident trace.",
    output: "incident data from /views/public/incidents, /views/projects/{projectId}/incidents, /views/incidents/{incidentId}, or /projects/{project_id}/incidents/stats",
    output_policy: WorkflowOutputPolicy::MachineRaw,
    legacy_examples: &[],
    actions: &[
        action!("list_public", false, "GET", "/views/public/incidents", "pcl incidents --limit 5", optional: ["--page", "--limit", "--network", "--sort", "--dev-mode", "--all", "--max-pages", "--output"]),
        action!("list_project", true, "GET", "/views/projects/{projectId}/incidents", "pcl incidents --project <project-ref> --all --limit 50 --output incidents.json", required: ["--project"], optional: ["--page", "--limit", "--assertion-id", "--adopter-id", "--environment", "--from", "--to", "--all", "--max-pages", "--output"]),
        action!("stats", true, "GET", "/projects/{project_id}/incidents/stats", "pcl incidents --project <project-ref> --stats", required: ["--project"]),
        action!("detail", true, "GET", "/views/incidents/{incidentId}", "pcl incidents --incident-id <incident-id>", required: ["--incident-id"]),
        action!("trace", true, "GET", "/views/incidents/{incidentId}/transactions/{txId}/trace", "pcl incidents --incident-id <incident-id> --tx-id <invalidating-transaction-id>", required: ["--incident-id", "--tx-id"]),
        action!("retry_trace", true, "POST", "/incidents/{incident_id}/transactions/{tx_id}/trace/retry", "pcl incidents --incident-id <incident-id> --tx-id <tx-id> --retry-trace", required: ["--incident-id", "--tx-id"], body_template: "empty_object"),
    ],
};
