use super::{
    AccessArgs,
    ContractsArgs,
    DeploymentsArgs,
    IntegrationsArgs,
    ProjectsArgs,
    ProtocolManagerArgs,
    ReleasesArgs,
    TransfersArgs,
    with_envelope_metadata,
};
use serde_json::{
    Value,
    json,
};

pub(super) fn template_envelope(data: Value) -> Value {
    let next_actions = if data
        .get("body_variants")
        .and_then(Value::as_array)
        .is_some_and(|variants| !variants.is_empty())
    {
        vec![
            "Choose one entry from data.body_variants and pass only its body with --body-file <path>",
            "Or pass fields from the chosen variant body with --field key=value",
        ]
    } else {
        vec![
            "Pass the template with --body-file <path>",
            "Or pass individual fields with --field key=value",
        ]
    };
    with_envelope_metadata(json!({
        "status": "ok",
        "data": data,
        "next_actions": next_actions,
    }))
}

pub(super) fn project_body_template(args: &ProjectsArgs) -> Value {
    if args.update {
        return body_template("project_update");
    }
    if args.save || args.unsave {
        return body_template("project_saved");
    }
    if args.delete || args.resolve || args.widget || args.mine || args.saved {
        return body_template("empty_object");
    }
    body_template("project_create")
}

pub(super) fn contracts_body_template(args: &ContractsArgs) -> Value {
    if args.assign_project {
        return body_template("contracts_assign_project");
    }
    if args.unassigned || args.remove || args.remove_calldata || args.adopter_id.is_some() {
        return body_template("empty_object");
    }
    body_template("contracts")
}

pub(super) fn release_body_template(args: &ReleasesArgs) -> Value {
    if args.deploy {
        return body_template("release_deploy");
    }
    if args.remove {
        return body_template("release_remove");
    }
    if args.deploy_calldata
        || args.remove_calldata
        || args.backtest_progress
        || args.retry_check
        || args.release_id.is_some()
    {
        return body_template("empty_object");
    }
    body_template("release")
}

pub(super) fn deployment_body_template(args: &DeploymentsArgs) -> Value {
    if !args.confirm {
        return body_template("empty_object");
    }
    body_template("deployment_confirmation")
}

pub(super) fn access_body_template(args: &AccessArgs) -> Value {
    if args.update_role {
        return body_template("role_update");
    }
    if args.invite {
        return body_template("access_invite");
    }
    if args.accept
        || args.resend
        || args.revoke
        || args.remove
        || args.members
        || args.invitations
        || args.pending
        || args.preview
        || args.my_role
    {
        return body_template("empty_object");
    }
    body_template("access_invite")
}

pub(super) fn integration_body_template(args: &IntegrationsArgs) -> Value {
    if args.test || args.delete {
        return body_template("empty_object");
    }
    if let Some(provider) = args.provider {
        return body_template(provider.path());
    }
    json!({
        "body_variants": [
            {
                "name": "slack",
                "body": body_template("slack")
            },
            {
                "name": "pagerduty",
                "body": body_template("pagerduty")
            }
        ]
    })
}

pub(super) fn protocol_manager_body_template(args: &ProtocolManagerArgs) -> Value {
    if args.set {
        return body_template("protocol_manager_set");
    }
    if args.confirm_transfer {
        return body_template("protocol_manager_confirm");
    }
    if args.clear
        || args.nonce
        || args.transfer_calldata
        || args.accept_calldata
        || args.pending_transfer
    {
        return body_template("empty_object");
    }
    body_template("protocol_manager_set")
}

pub(super) fn transfer_body_template(args: &TransfersArgs) -> Value {
    if !args.reject {
        return body_template("empty_object");
    }
    body_template("transfer_reject")
}

pub(super) fn body_template(kind: &str) -> Value {
    match kind {
        "project_create" => {
            json!({
                "project_name": "<name>",
                "chain_id": 1,
                "project_description": "<description>",
                "profile_image_url": "https://example.com/project.png",
                "is_private": false
            })
        }
        "project_update" => {
            json!({
                "project_name": "<name>",
                "project_description": "<description>",
                "github_url": "https://github.com/org/repo",
                "profile_image_url": "https://example.com/project.png",
                "is_dev": false,
                "is_private": false,
                "assertion_adopters": []
            })
        }
        "project_saved" => json!({ "project_id": "<project-uuid>" }),
        "release" => {
            json!({
                "environment": "staging",
                "assertionsDir": "assertions",
                "contracts": {
                    "<contract-key>": {
                        "address": "0x...",
                        "name": "<contract-name>",
                        "assertions": [
                            {
                                "file": "Assertion.sol",
                                "args": [],
                                "bytecode": "0x...",
                                "flattenedSource": "<source>",
                                "compilerVersion": "0.8.28",
                                "contractName": "<assertion-contract>",
                                "evmVersion": "paris",
                                "optimizerRuns": 200,
                                "optimizerEnabled": true,
                                "metadataBytecodeHash": "none",
                                "libraries": {}
                            }
                        ]
                    }
                },
                "compilerArgs": []
            })
        }
        "access_invite" => {
            json!({
                "identifier": "user@example.com",
                "identifier_type": "email",
                "role": "viewer"
            })
        }
        "role_update" => json!({ "role": "viewer" }),
        "release_deploy" => {
            json!({
                "chainId": 1,
                "txHash": "0x..."
            })
        }
        "release_remove" => {
            json!({
                "chainId": 1,
                "txHash": "0x..."
            })
        }
        "deployment_confirmation" => {
            json!({
                "tx_hash": "0x...",
                "chainId": 1,
                "environment": "staging",
                "assertions": [
                    {
                        "assertion_id": "0x...",
                        "assertion_adopters": [
                            {
                                "id": "<adopter-id>"
                            }
                        ]
                    }
                ]
            })
        }
        "slack" => {
            json!({
                "webhook_url": "https://hooks.slack.com/services/...",
                "enabled": true
            })
        }
        "pagerduty" => {
            json!({
                "routing_key": "<pagerduty-routing-key>",
                "enabled": true
            })
        }
        "protocol_manager_set" => {
            json!({
                "address": "0x...",
                "signature": "0x...",
                "nonce": "<nonce>"
            })
        }
        "protocol_manager_confirm" => {
            json!({
                "body_variants": [
                    {
                        "name": "direct",
                        "body": {
                            "mode": "direct",
                            "new_manager_address": "0x..."
                        }
                    },
                    {
                        "name": "onchain",
                        "body": {
                            "mode": "onchain",
                            "new_manager_address": "0x...",
                            "chain_id": 1,
                            "tx_hash": "0x..."
                        }
                    }
                ]
            })
        }
        "transfer_reject" => {
            json!({
                "ponder_transfer_id": "<transfer-id>"
            })
        }
        "contracts" => {
            json!({
                "network": "1",
                "address": "0x...",
                "contract_name": "<contract-name>",
                "project_id": "<project-uuid>"
            })
        }
        "contracts_assign_project" => {
            json!({
                "project_id": "<project-uuid>",
                "assertion_adopter_ids": ["<adopter-id>"]
            })
        }
        "empty_object" => json!({}),
        _ => json!({}),
    }
}
