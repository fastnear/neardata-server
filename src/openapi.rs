use std::path::PathBuf;

use anyhow::Result;
use fastnear_openapi_generator::{write_or_check_yaml, SchemaRegistry};
use serde_json::{json, Value};

use crate::types::{BlockErrorResponse, HealthResponse};

const API_VERSION: &str = "3.0.3";

pub fn generate(check: bool) -> Result<()> {
    let output_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("openapi");
    let mut registry = SchemaRegistry::openapi3();
    let health = registry.schema_ref::<HealthResponse>();
    let block_error = registry.schema_ref::<BlockErrorResponse>();
    let components = registry.into_components();

    let doc = json!({
        "openapi": API_VERSION,
        "info": {
            "title": "NEAR Data API",
            "version": API_VERSION,
            "description": "Cached and archived NEAR block data with redirect helpers for first-block and latest-block workflows. Some block-family routes may redirect depending on archive or freshness topology."
        },
        "servers": [
            {
                "url": "https://mainnet.neardata.xyz",
                "description": "Mainnet"
            },
            {
                "url": "https://testnet.neardata.xyz",
                "description": "Testnet"
            }
        ],
        "paths": {
            "/health": {
                "get": health_operation(health)
            },
            "/v0/first_block": {
                "get": first_block_operation()
            },
            "/v0/block/{block_height}": {
                "get": block_operation(
                    "block",
                    "NEAR Data API - Block",
                    "get_block",
                    "Fetch a finalized block by height",
                    "Fetch the finalized block document for a given height, including all chunks and shard data.",
                    block_document_schema(),
                    vec![block_height_parameter()],
                    block_error.clone()
                )
            },
            "/v0/block/{block_height}/headers": {
                "get": block_operation(
                    "block_headers",
                    "NEAR Data API - Block Headers",
                    "get_block_headers",
                    "Fetch the block-level object for a finalized block",
                    "Fetch the top-level block object for a finalized block, including the block header and chunk summaries.",
                    headers_document_schema(),
                    vec![block_height_parameter()],
                    block_error.clone()
                )
            },
            "/v0/block/{block_height}/chunk/{shard_id}": {
                "get": block_operation(
                    "block_chunk",
                    "NEAR Data API - Block Chunk",
                    "get_chunk",
                    "Fetch one chunk from a finalized block",
                    "Fetch a single chunk for a given finalized block and shard ID.",
                    chunk_document_schema(),
                    vec![block_height_parameter(), shard_id_parameter("Shard ID whose chunk should be returned.")],
                    block_error.clone()
                )
            },
            "/v0/block/{block_height}/shard/{shard_id}": {
                "get": block_operation(
                    "block_shard",
                    "NEAR Data API - Block Shard",
                    "get_shard",
                    "Fetch one shard from a finalized block",
                    "Fetch a single shard document for a given finalized block and shard ID.",
                    shard_document_schema(),
                    vec![block_height_parameter(), shard_id_parameter("Shard ID to return.")],
                    block_error.clone()
                )
            },
            "/v0/block_opt/{block_height}": {
                "get": block_operation(
                    "block_optimistic",
                    "NEAR Data API - Optimistic Block",
                    "get_block_optimistic",
                    "Fetch an optimistic block by height",
                    "Fetch an optimistic block document when the endpoint can serve it directly, or follow the documented redirect behavior when the deployment hands off to finalized or archive infrastructure.",
                    block_document_schema(),
                    vec![block_height_parameter()],
                    block_error.clone()
                )
            },
            "/v0/last_block/final": {
                "get": last_block_operation(
                    "last_block_final",
                    "NEAR Data API - Last Final Block",
                    "get_last_block_final",
                    "Redirect to the latest finalized block",
                    "This endpoint redirects to the latest finalized block URL for the active deployment."
                )
            },
            "/v0/last_block/optimistic": {
                "get": last_block_operation(
                    "last_block_optimistic",
                    "NEAR Data API - Last Optimistic Block",
                    "get_last_block_optimistic",
                    "Redirect to the latest optimistic block",
                    "This endpoint redirects to the latest optimistic block URL for the active deployment."
                )
            }
        },
        "components": {
            "schemas": components
        }
    });

    write_or_check_yaml(output_root.join("openapi.yaml"), &doc, check)?;
    Ok(())
}

fn health_operation(health_schema: Value) -> Value {
    json!({
        "operationId": "get_health",
        "summary": "Get service health",
        "description": "Check whether the neardata service is healthy for the current deployment.",
        "tags": ["system"],
        "x-fastnear-slug": "health",
        "x-fastnear-title": "NEAR Data API - Health",
        "parameters": [api_key_parameter()],
        "responses": {
            "200": json_response(
                "Health payload",
                health_schema,
                Some(json!({ "status": "ok" }))
            ),
            "401": plain_text_response("Invalid or unauthorized API key", Some("Unauthorized")),
            "500": json_string_response("Cache or internal data error")
        }
    })
}

fn first_block_operation() -> Value {
    json!({
        "operationId": "get_first_block",
        "summary": "Redirect to the first block after genesis",
        "description": "This endpoint redirects to the canonical first block URL for the selected network.",
        "tags": ["blocks"],
        "x-fastnear-slug": "first_block",
        "x-fastnear-title": "NEAR Data API - First Block",
        "parameters": [api_key_parameter()],
        "responses": {
            "200": json_response(
                "Full block document returned after automatic redirect following",
                block_document_schema(),
                None
            ),
            "302": redirect_response("Redirect to the canonical first block URL"),
            "401": plain_text_response("Invalid or unauthorized API key", Some("Unauthorized"))
        }
    })
}

fn last_block_operation(
    slug: &str,
    title: &str,
    operation_id: &str,
    summary: &str,
    description: &str,
) -> Value {
    json!({
        "operationId": operation_id,
        "summary": summary,
        "description": description,
        "tags": ["blocks"],
        "x-fastnear-slug": slug,
        "x-fastnear-title": title,
        "parameters": [api_key_parameter()],
        "responses": {
            "200": json_response(
                "Full block document returned after automatic redirect following",
                block_document_schema(),
                None
            ),
            "302": redirect_response(format!("Redirect to the latest {} block URL", summary.split_whitespace().last().unwrap_or("block")).as_str()),
            "401": plain_text_response("Invalid or unauthorized API key", Some("Unauthorized")),
            "500": json_string_response("Cache or internal data error")
        }
    })
}

fn block_operation(
    slug: &str,
    title: &str,
    operation_id: &str,
    summary: &str,
    description: &str,
    response_schema: Value,
    mut parameters: Vec<Value>,
    block_error_schema: Value,
) -> Value {
    parameters.push(api_key_parameter());
    json!({
        "operationId": operation_id,
        "summary": summary,
        "description": description,
        "tags": ["blocks"],
        "x-fastnear-slug": slug,
        "x-fastnear-title": title,
        "parameters": parameters,
        "responses": {
            "200": json_response(
                "Requested document, or `null` when the selected slice is absent",
                response_schema,
                None
            ),
            "302": redirect_response("Redirect to a canonical archive or finalized block URL"),
            "401": plain_text_response("Invalid or unauthorized API key", Some("Unauthorized")),
            "404": json_response(
                "Structured block-height error",
                block_error_schema,
                Some(json!({
                    "error": "The block does not exist in this archive range",
                    "type": "BLOCK_DOES_NOT_EXIST"
                }))
            ),
            "500": json_string_response("Cache or internal data error")
        }
    })
}

fn json_response(description: &str, schema: Value, example: Option<Value>) -> Value {
    let mut content = json!({
        "application/json": {
            "schema": schema
        }
    });

    if let Some(example) = example {
        content["application/json"]["example"] = example;
    }

    json!({
        "description": description,
        "content": content
    })
}

fn json_string_response(description: &str) -> Value {
    json_response(description, json!({ "type": "string" }), None)
}

fn plain_text_response(description: &str, example: Option<&str>) -> Value {
    let mut content = json!({
        "text/plain": {
            "schema": {
                "type": "string"
            }
        }
    });

    if let Some(example) = example {
        content["text/plain"]["example"] = json!(example);
    }

    json!({
        "description": description,
        "content": content
    })
}

fn redirect_response(description: &str) -> Value {
    json!({
        "description": description,
        "headers": {
            "Location": {
                "schema": {
                    "type": "string"
                }
            }
        }
    })
}

fn block_document_schema() -> Value {
    raw_json_object_or_null(
        "Full block document as served by neardata, including `block` and `shards`.",
    )
}

fn headers_document_schema() -> Value {
    raw_json_object_or_null(
        "Block-level object returned by `/headers`, corresponding to the full response's `block` field.",
    )
}

fn shard_document_schema() -> Value {
    raw_json_object_or_null("Shard object for the requested shard ID.")
}

fn chunk_document_schema() -> Value {
    raw_json_object_or_null("Chunk object for the requested shard ID.")
}

fn raw_json_object_or_null(description: &str) -> Value {
    json!({
        "type": "object",
        "nullable": true,
        "additionalProperties": true,
        "description": description
    })
}

fn api_key_parameter() -> Value {
    json!({
        "name": "apiKey",
        "in": "query",
        "required": false,
        "description": "Optional FastNEAR subscription API key. Invalid values may return `401` before redirect handling.",
        "schema": {
            "type": "string"
        }
    })
}

fn block_height_parameter() -> Value {
    json!({
        "name": "block_height",
        "in": "path",
        "required": true,
        "description": "NEAR block height to retrieve.",
        "schema": {
            "type": "string"
        },
        "example": "9820210"
    })
}

fn shard_id_parameter(description: &str) -> Value {
    json!({
        "name": "shard_id",
        "in": "path",
        "required": true,
        "description": description,
        "schema": {
            "type": "string"
        },
        "example": "0"
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        block_document_schema, block_height_parameter, block_operation, headers_document_schema,
        raw_json_object_or_null,
    };

    #[test]
    fn raw_json_schemas_stay_nullable_objects() {
        let schema = raw_json_object_or_null("Example");

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["nullable"], true);
        assert_eq!(schema["additionalProperties"], true);
    }

    #[test]
    fn block_schema_fragments_preserve_object_or_null_shape() {
        assert_eq!(block_document_schema()["nullable"], true);
        assert_eq!(headers_document_schema()["type"], "object");
    }

    #[test]
    fn block_routes_document_redirect_auth_and_not_found_statuses() {
        let doc = block_operation(
            "block",
            "NEAR Data API - Block",
            "get_block",
            "Fetch a finalized block by height",
            "Example description",
            block_document_schema(),
            vec![block_height_parameter()],
            json!({
                "$ref": "#/components/schemas/BlockErrorResponse"
            }),
        );
        let responses = &doc["responses"];

        assert!(responses["200"].is_object());
        assert!(responses["302"].is_object());
        assert!(responses["401"].is_object());
        assert!(responses["404"].is_object());
        assert!(responses["500"].is_object());
    }

    #[test]
    fn block_error_schema_examples_preserve_wire_enum_values() {
        let doc = block_operation(
            "block",
            "NEAR Data API - Block",
            "get_block",
            "Fetch a finalized block by height",
            "Example description",
            block_document_schema(),
            vec![block_height_parameter()],
            json!({
                "$ref": "#/components/schemas/BlockErrorResponse"
            }),
        );

        assert_eq!(
            doc["responses"]["404"]["content"]["application/json"]["example"],
            json!({
                "error": "The block does not exist in this archive range",
                "type": "BLOCK_DOES_NOT_EXIST"
            })
        );
    }
}
