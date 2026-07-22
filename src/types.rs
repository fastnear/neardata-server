use std::fmt::Display;

use serde::Serialize;

pub type BlockHeight = u64;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ChainId {
    Mainnet,
    Testnet,
}

impl Display for ChainId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChainId::Mainnet => write!(f, "mainnet"),
            ChainId::Testnet => write!(f, "testnet"),
        }
    }
}

impl TryFrom<String> for ChainId {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "mainnet" => Ok(ChainId::Mainnet),
            "testnet" => Ok(ChainId::Testnet),
            _ => Err(format!("Invalid chain id: {}", value)),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Finality {
    Final,
    Optimistic,
}

impl Display for Finality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Finality::Final => write!(f, "final"),
            Finality::Optimistic => write!(f, "optimistic"),
        }
    }
}

impl TryFrom<String> for Finality {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "final" => Ok(Finality::Final),
            "optimistic" => Ok(Finality::Optimistic),
            _ => Err(format!("Invalid finality: {}", value)),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", schemars(deny_unknown_fields))]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "openapi", schemars(deny_unknown_fields))]
pub struct BlockErrorResponse {
    pub error: String,
    #[serde(rename = "type")]
    pub error_type: BlockErrorType,
}

impl BlockErrorResponse {
    pub fn new(error: impl Into<String>, error_type: BlockErrorType) -> Self {
        Self {
            error: error.into(),
            error_type,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[cfg_attr(feature = "openapi", derive(schemars::JsonSchema))]
pub enum BlockErrorType {
    BlockHeightTooHigh,
    BlockHeightTooLow,
    BlockDoesNotExist,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{BlockErrorResponse, BlockErrorType, HealthResponse};

    #[test]
    fn health_response_serializes_current_wire_shape() {
        let response = HealthResponse {
            status: "ok".to_string(),
        };

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({ "status": "ok" })
        );
    }

    #[test]
    fn block_error_response_uses_current_field_names_and_values() {
        let response = BlockErrorResponse::new(
            "The block is too far in the future",
            BlockErrorType::BlockDoesNotExist,
        );

        assert_eq!(
            serde_json::to_value(response).unwrap(),
            json!({
                "error": "The block is too far in the future",
                "type": "BLOCK_DOES_NOT_EXIST"
            })
        );
    }
}
