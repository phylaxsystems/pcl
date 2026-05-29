#[path = "../build/spec_transform.rs"]
mod spec_transform;

use serde_json::json;

#[test]
fn normalizes_standard_inline_error_responses_to_named_ref() {
    let mut spec = json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Example",
            "version": "1.0.0"
        },
        "paths": {
            "/auth/refresh": {
                "post": {
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "token": { "type": "string" }
                                        }
                                    }
                                }
                            }
                        },
                        "401": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "error": { "type": "string" },
                                            "code": { "type": "string" },
                                            "details": { "type": "string" }
                                        },
                                        "required": ["error"]
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    spec_transform::normalize_error_response_schemas(&mut spec);

    assert_eq!(
        spec["paths"]["/auth/refresh"]["post"]["responses"]["401"]["content"]["application/json"]["schema"],
        json!({ "$ref": "#/components/schemas/ApiError" })
    );
    assert_eq!(
        spec["paths"]["/auth/refresh"]["post"]["responses"]["200"]["content"]["application/json"]["schema"]
            ["properties"]["token"]["type"],
        "string"
    );
    assert_eq!(
        spec["components"]["schemas"]["ApiError"]["required"],
        json!(["error"])
    );
}

#[test]
fn leaves_nonstandard_error_responses_inline() {
    let mut spec = json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Example",
            "version": "1.0.0"
        },
        "paths": {
            "/items": {
                "get": {
                    "responses": {
                        "422": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "violations": {
                                                "type": "array",
                                                "items": { "type": "string" }
                                            }
                                        },
                                        "required": ["violations"]
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    spec_transform::normalize_error_response_schemas(&mut spec);

    assert_eq!(
        spec["paths"]["/items"]["get"]["responses"]["422"]["content"]["application/json"]["schema"]
            ["properties"]["violations"]["type"],
        "array"
    );
}

#[test]
fn normalizes_message_code_error_responses_to_named_ref() {
    let mut spec = json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Example",
            "version": "1.0.0"
        },
        "paths": {
            "/admin/reviewables": {
                "get": {
                    "responses": {
                        "403": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "properties": {
                                            "code": {
                                                "type": "string",
                                                "enum": ["forbidden"]
                                            },
                                            "message": { "type": "string" }
                                        },
                                        "required": ["code", "message"]
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    spec_transform::normalize_error_response_schemas(&mut spec);

    assert_eq!(
        spec["paths"]["/admin/reviewables"]["get"]["responses"]["403"]["content"]["application/json"]
            ["schema"],
        json!({ "$ref": "#/components/schemas/ApiMessageError" })
    );
    assert_eq!(
        spec["components"]["schemas"]["ApiMessageError"]["required"],
        json!(["code", "message"])
    );
}

#[test]
fn retain_client_operations_prunes_paths_and_unreachable_components() {
    let mut spec = json!({
        "openapi": "3.0.0",
        "info": {
            "title": "Example",
            "version": "1.0.0"
        },
        "paths": {
            "/health": {
                "get": {
                    "operationId": "get_health",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Health" }
                                }
                            }
                        }
                    }
                }
            },
            "/projects": {
                "get": {
                    "operationId": "get_projects",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "array",
                                        "items": { "$ref": "#/components/schemas/Project" }
                                    }
                                }
                            }
                        }
                    }
                }
            },
            "/admin": {
                "get": {
                    "operationId": "get_admin",
                    "responses": {
                        "200": {
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/Admin" }
                                }
                            }
                        }
                    }
                }
            }
        },
        "components": {
            "schemas": {
                "Health": { "type": "object" },
                "Project": {
                    "type": "object",
                    "properties": {
                        "owner": { "$ref": "#/components/schemas/User" }
                    }
                },
                "User": { "type": "object" },
                "Admin": { "type": "object" }
            }
        }
    });

    spec_transform::retain_client_operations(&mut spec, &["get_projects"]);

    assert!(spec["paths"].get("/health").is_none());
    assert!(spec["paths"].get("/admin").is_none());
    assert!(spec["paths"].get("/projects").is_some());
    assert!(spec["components"]["schemas"].get("Project").is_some());
    assert!(spec["components"]["schemas"].get("User").is_some());
    assert!(spec["components"]["schemas"].get("Health").is_none());
    assert!(spec["components"]["schemas"].get("Admin").is_none());
}
