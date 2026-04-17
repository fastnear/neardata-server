use std::path::PathBuf;

use anyhow::Result;
use fastnear_openapi_generator::{write_or_check_yaml, SchemaRegistry};
use serde_json::{json, Map, Value};

use crate::types::{BlockErrorResponse, HealthResponse};

const API_VERSION: &str = "3.0.3";

pub fn generate(check: bool) -> Result<()> {
    let output_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("openapi");
    let mut registry = SchemaRegistry::openapi3();
    let health = registry.schema_ref::<HealthResponse>();
    let block_error = registry.schema_ref::<BlockErrorResponse>();
    let mut components = registry.into_components();
    insert_generated_schemas(
        components
            .as_object_mut()
            .expect("schema registry should serialize as a JSON object"),
    );

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
                    "Fetch a finalized block's full document at a chosen height — header plus every chunk and shard payload.",
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
                    "Fetch only a finalized block's header and chunk summaries — no per-shard payload.",
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
                    "Fetch one chunk — a single shard's transactions and incoming receipts — at a chosen block height.",
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
                    "Fetch one shard's full payload at a chosen block — chunk plus state changes and produced receipts.",
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
                    "Fetch an optimistic (not-yet-final) block at a chosen height — may redirect once the optimistic window has finalized.",
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
                    "Redirect to the most recent finalized block — the chain-tip cursor once consensus has settled."
                )
            },
            "/v0/last_block/optimistic": {
                "get": last_block_operation(
                    "last_block_optimistic",
                    "NEAR Data API - Last Optimistic Block",
                    "get_last_block_optimistic",
                    "Redirect to the latest optimistic block",
                    "Redirect to the most recent optimistic block — the freshest-possible tip, ahead of final settlement."
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
        "description": "Ping the neardata service for liveness — returns `{status: ok}` when healthy, errors otherwise.",
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
        "description": "Redirect to the chain's first post-genesis block — a starting cursor for indexers backfilling from the beginning.",
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
    component_ref_or_null("BlockDocument")
}

fn headers_document_schema() -> Value {
    component_ref_or_null("BlockEnvelope")
}

fn shard_document_schema() -> Value {
    component_ref_or_null("ShardDocument")
}

fn chunk_document_schema() -> Value {
    component_ref_or_null("ChunkDocument")
}

fn component_ref(name: &str) -> Value {
    json!({
        "$ref": format!("#/components/schemas/{name}")
    })
}

fn component_ref_or_null(name: &str) -> Value {
    json!({
        "nullable": true,
        "$ref": format!("#/components/schemas/{name}")
    })
}

fn insert_generated_schemas(components: &mut Map<String, Value>) {
    for (name, schema) in [
        ("BlockDocument", block_document_component()),
        ("BlockEnvelope", block_envelope_component()),
        ("BlockHeader", block_header_component()),
        ("ChunkDocument", chunk_document_component()),
        ("ChunkHeader", chunk_header_component()),
        ("ChunkTransactionWrapper", chunk_transaction_wrapper_component()),
        ("ActionDocument", action_document_component()),
        ("ActionReceiptBody", action_receipt_body_component()),
        ("ActionReceiptDocument", action_receipt_document_component()),
        ("DataReceiptBody", data_receipt_body_component()),
        ("DataReceiptDocument", data_receipt_document_component()),
        ("ExecutionWithReceipt", execution_with_receipt_component()),
        ("ExecutionOutcomeDocument", execution_outcome_document_component()),
        ("ExecutionOutcomeStatus", execution_outcome_status_component()),
        (
            "ExecutionOutcomeStatusFailure",
            execution_outcome_status_failure_component(),
        ),
        (
            "ExecutionOutcomeStatusSuccessReceiptId",
            execution_outcome_status_success_receipt_id_component(),
        ),
        (
            "ExecutionOutcomeStatusSuccessValue",
            execution_outcome_status_success_value_component(),
        ),
        ("ExecutionOutcomeSummary", execution_outcome_summary_component()),
        ("ExecutionProofItem", execution_proof_item_component()),
        ("OmittedReceiptDocument", omitted_receipt_document_component()),
        (
            "OutputDataReceiverDocument",
            output_data_receiver_document_component(),
        ),
        ("ReceiptBody", receipt_body_component()),
        ("ReceiptDocument", receipt_document_component()),
        ("ShardDocument", shard_document_component()),
        ("StateChangeItem", state_change_item_component()),
        ("SignedTransactionDocument", signed_transaction_document_component()),
        ("StateChangeCause", state_change_cause_component()),
        (
            "StateChangeCauseActionReceiptGasReward",
            state_change_cause_action_receipt_gas_reward_component(),
        ),
        (
            "StateChangeCauseReceiptProcessing",
            state_change_cause_receipt_processing_component(),
        ),
        (
            "StateChangeCauseTransactionProcessing",
            state_change_cause_transaction_processing_component(),
        ),
        ("StateChangeValue", state_change_value_component()),
        (
            "StateChangeValueAccessKeyUpdate",
            state_change_value_access_key_update_component(),
        ),
        (
            "StateChangeValueAccountUpdate",
            state_change_value_account_update_component(),
        ),
        (
            "StateChangeValueDataDeletion",
            state_change_value_data_deletion_component(),
        ),
        (
            "StateChangeValueDataUpdate",
            state_change_value_data_update_component(),
        ),
    ] {
        components.insert(name.to_string(), schema);
    }
}

fn block_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Full block document as served by neardata, including the block envelope and per-shard payloads.",
        "properties": {
            "block": component_ref("BlockEnvelope"),
            "shards": {
                "type": "array",
                "items": component_ref("ShardDocument")
            }
        },
        "required": ["block", "shards"]
    })
}

fn block_envelope_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Block-level payload returned by neardata.",
        "properties": {
            "author": {
                "type": "string",
                "description": "Block producer account ID."
            },
            "chunks": {
                "type": "array",
                "items": component_ref("ChunkHeader")
            },
            "header": component_ref("BlockHeader")
        },
        "required": ["author", "chunks", "header"]
    })
}

fn block_header_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "description": "Block header object as served by neardata.",
        "properties": {
            "chunks_included": { "type": "integer", "format": "uint64" },
            "epoch_id": { "type": "string" },
            "gas_price": { "type": "string" },
            "hash": { "type": "string" },
            "height": { "type": "integer", "format": "uint64" },
            "next_epoch_id": { "type": "string" },
            "prev_hash": { "type": "string" },
            "prev_height": { "type": "integer", "format": "uint64" },
            "timestamp": { "type": "integer" },
            "timestamp_nanosec": { "type": "string" },
            "total_supply": { "type": "string" }
        }
    })
}

fn chunk_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Chunk payload returned by neardata for a single shard in a selected block.",
        "properties": {
            "author": {
                "type": "string",
                "description": "Chunk producer account ID."
            },
            "header": component_ref("ChunkHeader"),
            "receipts": {
                "type": "array",
                "items": component_ref("ReceiptDocument")
            },
            "transactions": {
                "type": "array",
                "items": component_ref("ChunkTransactionWrapper")
            }
        },
        "required": ["author", "header", "receipts", "transactions"]
    })
}

fn chunk_header_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true,
        "description": "Chunk header object as served by neardata.",
        "properties": {
            "chunk_hash": { "type": "string" },
            "gas_limit": { "type": "integer" },
            "gas_used": { "type": "integer" },
            "height_created": { "type": "integer", "format": "uint64" },
            "height_included": { "type": "integer", "format": "uint64" },
            "outcome_root": { "type": "string" },
            "outgoing_receipts_root": { "type": "string" },
            "prev_block_hash": { "type": "string" },
            "shard_id": { "type": "integer", "format": "uint64" },
            "tx_root": { "type": "string" }
        }
    })
}

fn chunk_transaction_wrapper_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Transaction entry returned inside a neardata chunk.",
        "properties": {
            "outcome": component_ref("ExecutionWithReceipt"),
            "transaction": component_ref("SignedTransactionDocument")
        },
        "required": ["outcome", "transaction"]
    })
}

fn action_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true
    })
}

fn action_receipt_body_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "Action": component_ref("ActionReceiptDocument")
        },
        "required": ["Action"]
    })
}

fn action_receipt_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "actions": {
                "type": "array",
                "items": component_ref("ActionDocument")
            },
            "gas_price": { "type": "string" },
            "input_data_ids": {
                "type": "array",
                "items": { "type": "string" }
            },
            "is_promise_yield": { "type": "boolean" },
            "output_data_receivers": {
                "type": "array",
                "items": component_ref("OutputDataReceiverDocument")
            },
            "signer_id": { "type": "string" },
            "signer_public_key": { "type": "string" }
        },
        "required": [
            "actions",
            "gas_price",
            "input_data_ids",
            "is_promise_yield",
            "output_data_receivers",
            "signer_id",
            "signer_public_key"
        ]
    })
}

fn data_receipt_body_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "Data": component_ref("DataReceiptDocument")
        },
        "required": ["Data"]
    })
}

fn data_receipt_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "data": { "type": "string" },
            "data_id": { "type": "string" },
            "is_promise_resume": { "type": "boolean" }
        },
        "required": ["data", "data_id", "is_promise_resume"]
    })
}

fn execution_with_receipt_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Execution result paired with an optional receipt object.",
        "properties": {
            "execution_outcome": component_ref("ExecutionOutcomeDocument"),
            "receipt": {
                "type": "object",
                "nullable": true,
                "description": "Receipt payload when neardata includes it for this entry.",
                "oneOf": [
                    component_ref("ReceiptDocument"),
                    component_ref("OmittedReceiptDocument")
                ]
            },
            "tx_hash": { "type": "string" }
        },
        "required": ["execution_outcome", "receipt"]
    })
}

fn execution_outcome_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "block_hash": { "type": "string" },
            "id": { "type": "string" },
            "outcome": component_ref("ExecutionOutcomeSummary"),
            "proof": {
                "type": "array",
                "items": component_ref("ExecutionProofItem")
            }
        },
        "required": ["block_hash", "id", "outcome", "proof"]
    })
}

fn execution_outcome_status_component() -> Value {
    json!({
        "oneOf": [
            component_ref("ExecutionOutcomeStatusSuccessReceiptId"),
            component_ref("ExecutionOutcomeStatusSuccessValue"),
            component_ref("ExecutionOutcomeStatusFailure")
        ]
    })
}

fn execution_outcome_status_failure_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "Failure": {
                "type": "object",
                "additionalProperties": true
            }
        },
        "required": ["Failure"]
    })
}

fn execution_outcome_status_success_receipt_id_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "SuccessReceiptId": { "type": "string" }
        },
        "required": ["SuccessReceiptId"]
    })
}

fn execution_outcome_status_success_value_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "SuccessValue": { "type": "string" }
        },
        "required": ["SuccessValue"]
    })
}

fn execution_outcome_summary_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "executor_id": { "type": "string" },
            "gas_burnt": { "type": "integer", "format": "uint64" },
            "logs": {
                "type": "array",
                "items": { "type": "string" }
            },
            "metadata": {
                "type": "object",
                "additionalProperties": true
            },
            "receipt_ids": {
                "type": "array",
                "items": { "type": "string" }
            },
            "status": component_ref("ExecutionOutcomeStatus"),
            "tokens_burnt": { "type": "string" }
        },
        "required": [
            "executor_id",
            "gas_burnt",
            "logs",
            "metadata",
            "receipt_ids",
            "status",
            "tokens_burnt"
        ]
    })
}

fn execution_proof_item_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": true
    })
}

fn omitted_receipt_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false
    })
}

fn output_data_receiver_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "data_id": { "type": "string" },
            "receiver_id": { "type": "string" }
        },
        "required": ["data_id", "receiver_id"]
    })
}

fn receipt_body_component() -> Value {
    json!({
        "oneOf": [
            component_ref("ActionReceiptBody"),
            component_ref("DataReceiptBody")
        ]
    })
}

fn receipt_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Receipt object as served by neardata inside a chunk payload.",
        "properties": {
            "predecessor_id": { "type": "string" },
            "priority": { "type": "integer", "format": "uint64" },
            "receipt": component_ref("ReceiptBody"),
            "receipt_id": { "type": "string" },
            "receiver_id": { "type": "string" }
        },
        "required": [
            "predecessor_id",
            "priority",
            "receipt",
            "receipt_id",
            "receiver_id"
        ]
    })
}

fn shard_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Per-shard payload returned by neardata for a block.",
        "properties": {
            "chunk": component_ref("ChunkDocument"),
            "receipt_execution_outcomes": {
                "type": "array",
                "items": component_ref("ExecutionWithReceipt")
            },
            "shard_id": { "type": "integer", "format": "uint64" },
            "state_changes": {
                "type": "array",
                "items": component_ref("StateChangeItem")
            }
        },
        "required": [
            "chunk",
            "receipt_execution_outcomes",
            "shard_id",
            "state_changes"
        ]
    })
}

fn state_change_item_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "State change entry returned by neardata for a shard.",
        "properties": {
            "cause": component_ref("StateChangeCause"),
            "change": component_ref("StateChangeValue"),
            "type": { "type": "string" }
        },
        "required": ["cause", "change", "type"]
    })
}

fn signed_transaction_document_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "actions": {
                "type": "array",
                "items": component_ref("ActionDocument")
            },
            "hash": { "type": "string" },
            "nonce": { "type": "integer", "format": "uint64" },
            "priority_fee": { "type": "integer", "format": "uint64" },
            "public_key": { "type": "string" },
            "receiver_id": { "type": "string" },
            "signature": { "type": "string" },
            "signer_id": { "type": "string" }
        },
        "required": [
            "actions",
            "hash",
            "nonce",
            "priority_fee",
            "public_key",
            "receiver_id",
            "signature",
            "signer_id"
        ]
    })
}

fn state_change_cause_component() -> Value {
    json!({
        "oneOf": [
            component_ref("StateChangeCauseTransactionProcessing"),
            component_ref("StateChangeCauseReceiptProcessing"),
            component_ref("StateChangeCauseActionReceiptGasReward")
        ]
    })
}

fn state_change_cause_action_receipt_gas_reward_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "receipt_hash": { "type": "string" },
            "type": { "type": "string" }
        },
        "required": ["receipt_hash", "type"]
    })
}

fn state_change_cause_receipt_processing_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "receipt_hash": { "type": "string" },
            "type": { "type": "string" }
        },
        "required": ["receipt_hash", "type"]
    })
}

fn state_change_cause_transaction_processing_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "tx_hash": { "type": "string" },
            "type": { "type": "string" }
        },
        "required": ["tx_hash", "type"]
    })
}

fn state_change_value_component() -> Value {
    json!({
        "oneOf": [
            component_ref("StateChangeValueAccountUpdate"),
            component_ref("StateChangeValueAccessKeyUpdate"),
            component_ref("StateChangeValueDataUpdate"),
            component_ref("StateChangeValueDataDeletion")
        ]
    })
}

fn state_change_value_access_key_update_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "access_key": {
                "type": "object",
                "additionalProperties": true
            },
            "account_id": { "type": "string" },
            "public_key": { "type": "string" }
        },
        "required": ["access_key", "account_id", "public_key"]
    })
}

fn state_change_value_account_update_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "account_id": { "type": "string" },
            "amount": { "type": "string" },
            "code_hash": { "type": "string" },
            "locked": { "type": "string" },
            "storage_paid_at": { "type": "integer", "format": "uint64" },
            "storage_usage": { "type": "integer", "format": "uint64" }
        },
        "required": [
            "account_id",
            "amount",
            "code_hash",
            "locked",
            "storage_paid_at",
            "storage_usage"
        ]
    })
}

fn state_change_value_data_deletion_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "account_id": { "type": "string" },
            "key_base64": { "type": "string" }
        },
        "required": ["account_id", "key_base64"]
    })
}

fn state_change_value_data_update_component() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "account_id": { "type": "string" },
            "key_base64": { "type": "string" },
            "value_base64": { "type": "string" }
        },
        "required": ["account_id", "key_base64", "value_base64"]
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
        "example": "50000000"
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
    };

    #[test]
    fn component_refs_stay_nullable() {
        let schema = block_document_schema();

        assert_eq!(schema["$ref"], "#/components/schemas/BlockDocument");
        assert_eq!(schema["nullable"], true);
    }

    #[test]
    fn block_schema_fragments_preserve_component_refs() {
        assert_eq!(block_document_schema()["nullable"], true);
        assert_eq!(
            headers_document_schema()["$ref"],
            "#/components/schemas/BlockEnvelope"
        );
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
