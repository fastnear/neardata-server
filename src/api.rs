use crate::cache::set_multiple_blocks_async;
use crate::reader::read_blocks;
use crate::types::*;
use crate::*;
use actix_web::ResponseError;
use serde_json::json;
use std::fmt;

const TARGET_API: &str = "api";
const MAX_BLOCK_HEIGHT: BlockHeight = 10u64.pow(15);
const EXPECTED_CACHED_BLOCKS: BlockHeight = 10;

#[derive(Debug)]
enum ServiceError {
    ArgumentError,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ServiceError::ArgumentError => write!(f, "Invalid argument"),
        }
    }
}

impl ResponseError for ServiceError {
    fn error_response(&self) -> HttpResponse {
        match *self {
            ServiceError::ArgumentError => HttpResponse::BadRequest().json("Invalid argument"),
        }
    }
}

pub mod v0 {
    use super::*;

    #[get("/block/{block_height}")]
    pub async fn get_block(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let chain_id = app_state.chain_id;
        let block_height = request
            .match_info()
            .get("block_height")
            .unwrap()
            .parse::<BlockHeight>()
            .map_err(|_| ServiceError::ArgumentError)?;
        if block_height > MAX_BLOCK_HEIGHT {
            return Ok(HttpResponse::NotFound().json(json!({
                "error": "Block height is too high",
                "type": "BLOCK_HEIGHT_TOO_HIGH"
            })));
        }

        tracing::debug!(target: TARGET_API, "Retrieving block for block_height: {}", block_height);

        let mut block =
            match cache::get_block(app_state.redis_client.clone(), chain_id, block_height).await {
                Some(block) => block,
                None => {
                    // Not cached

                    // Trying to check last block
                    if let Some(last_block_height) =
                        cache::get_last_block_height(app_state.redis_client.clone(), chain_id).await
                    {
                        if block_height > last_block_height.saturating_sub(EXPECTED_CACHED_BLOCKS) {
                            return Ok(HttpResponse::NotFound().json(json!({
                                "error": "Block doesn't exist yet",
                                "type": "BLOCK_DOES_NOT_EXIST"
                            })));
                        }
                    }

                    let blocks = read_blocks(&app_state.read_config, chain_id, block_height);
                    let block = blocks
                        .iter()
                        .find_map(|(height, block)| {
                            if *height == block_height {
                                Some(block.as_ref().cloned().unwrap_or_default())
                            } else {
                                None
                            }
                        })
                        .unwrap();
                    set_multiple_blocks_async(app_state.redis_client.clone(), chain_id, blocks);
                    block
                }
            };

        if block.is_empty() {
            block = "null".to_string();
        }
        Ok(HttpResponse::Ok()
            .append_header((header::CONTENT_TYPE, "application/json; charset=utf-8"))
            .body(block))
    }
}
