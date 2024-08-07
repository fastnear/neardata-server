use crate::cache::set_multiple_blocks_async;
use crate::reader::read_blocks;
use crate::types::*;
use crate::*;
use actix_web::ResponseError;
use serde_json::json;
use std::fmt;
use std::time::Duration;

const TARGET_API: &str = "api";
const MAX_BLOCK_HEIGHT: BlockHeight = 10u64.pow(15);
const EXPECTED_CACHED_BLOCKS: BlockHeight = 10;
// 1 year cache for blocks. Blocks don't change.
const DEFAULT_CACHE_DURATION: Duration = Duration::from_secs(365 * 24 * 60 * 60);
const MAX_WAIT_BLOCKS: BlockHeight = 10;

#[derive(Debug)]
enum ServiceError {
    ArgumentError,
    CacheError(String),
}

impl From<redis::RedisError> for ServiceError {
    fn from(_err: redis::RedisError) -> Self {
        ServiceError::CacheError("Redis error".to_string())
    }
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ServiceError::ArgumentError => write!(f, "Invalid argument"),
            ServiceError::CacheError(ref err) => write!(f, "Cache error: {}", err),
        }
    }
}

impl ResponseError for ServiceError {
    fn error_response(&self) -> HttpResponse {
        match *self {
            ServiceError::ArgumentError => HttpResponse::BadRequest().json("Invalid argument"),
            ServiceError::CacheError(ref err) => {
                HttpResponse::InternalServerError().json(format!("Cache error: {}", err))
            }
        }
    }
}

pub mod v0 {
    use super::*;
    use crate::cache::finality_suffix;

    #[get("/last_block/{finality}")]
    pub async fn get_last_block(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let chain_id = app_state.chain_id;
        let finality =
            Finality::try_from(request.match_info().get("finality").unwrap().to_string())
                .map_err(|_| ServiceError::ArgumentError)?;
        if !app_state.is_latest {
            // Redirect to the main url
            return Ok(HttpResponse::Found()
                .append_header((
                    header::LOCATION,
                    format!(
                        "{}/v0/last_block/{}",
                        app_state.archive_config.as_ref().unwrap().main_url,
                        finality
                    ),
                ))
                .finish());
        }

        tracing::debug!(target: TARGET_API, "Retrieving the last block for finality {}", finality);

        let last_block_height =
            cache::get_last_block_height(app_state.redis_client.clone(), chain_id, finality)
                .await
                .ok_or_else(|| {
                    ServiceError::CacheError(
                        "The last block height is missing from the cache".to_string(),
                    )
                })?;
        Ok(HttpResponse::Found()
            .append_header((
                header::LOCATION,
                format!(
                    "/v0/block{}/{}",
                    finality_suffix(finality),
                    last_block_height
                ),
            ))
            .finish())
    }

    #[get("/first_block")]
    pub async fn get_first_block(
        _request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        if let Some(archive_config) = &app_state.archive_config {
            if app_state.is_latest {
                return Ok(HttpResponse::Found()
                    .append_header((
                        header::CACHE_CONTROL,
                        format!("public, max-age={}", 24 * 60 * 60),
                    ))
                    .append_header((
                        header::LOCATION,
                        format!(
                            "{}/v0/block/{}",
                            archive_config.archive_url, app_state.genesis_block_height
                        ),
                    ))
                    .finish());
            }
        }
        Ok(HttpResponse::Found()
            .append_header((
                header::CACHE_CONTROL,
                format!("public, max-age={}", 24 * 60 * 60),
            ))
            .append_header((
                header::LOCATION,
                format!("/v0/block/{}", app_state.genesis_block_height),
            ))
            .finish())
    }

    #[get("/block_opt/{block_height}")]
    pub async fn get_opt_block(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let block_height = request
            .match_info()
            .get("block_height")
            .unwrap()
            .parse::<BlockHeight>()
            .map_err(|_| ServiceError::ArgumentError)?;
        get_block_inner(block_height, Finality::Optimistic, app_state).await
    }

    #[get("/block/{block_height}")]
    pub async fn get_block(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let block_height = request
            .match_info()
            .get("block_height")
            .unwrap()
            .parse::<BlockHeight>()
            .map_err(|_| ServiceError::ArgumentError)?;
        get_block_inner(block_height, Finality::Final, app_state).await
    }

    async fn get_block_inner(
        block_height: BlockHeight,
        finality: Finality,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let chain_id = app_state.chain_id;

        if block_height > MAX_BLOCK_HEIGHT {
            return Ok(HttpResponse::NotFound()
                .append_header((
                    header::CACHE_CONTROL,
                    format!("public, max-age={}", 24 * 60 * 60),
                ))
                .json(json!({
                    "error": "Block height is too high",
                    "type": "BLOCK_HEIGHT_TOO_HIGH"
                })));
        }
        if block_height < app_state.genesis_block_height {
            return Ok(HttpResponse::NotFound()
                .append_header((
                    header::CACHE_CONTROL,
                    format!("public, max-age={}", 24 * 60 * 60),
                ))
                .json(json!({
                    "error": "Block height is before the genesis",
                    "type": "BLOCK_HEIGHT_TOO_LOW"
                })));
        }

        if let Some(archive_config) = &app_state.archive_config {
            if app_state.is_latest && block_height < archive_config.end_height {
                return Ok(HttpResponse::Found()
                    .append_header((
                        header::CACHE_CONTROL,
                        format!("public, max-age={}", 24 * 60 * 60),
                    ))
                    .append_header((
                        header::LOCATION,
                        format!("{}/v0/block/{}", archive_config.archive_url, block_height),
                    ))
                    .finish());
            } else if !app_state.is_latest
                && (block_height >= archive_config.end_height || finality == Finality::Optimistic)
            {
                return Ok(HttpResponse::Found()
                    .append_header((
                        header::CACHE_CONTROL,
                        format!("public, max-age={}", 24 * 60 * 60),
                    ))
                    .append_header((
                        header::LOCATION,
                        format!(
                            "{}/v0/block{}/{}",
                            archive_config.main_url,
                            finality_suffix(finality),
                            block_height
                        ),
                    ))
                    .finish());
            }
        }

        tracing::debug!(target: TARGET_API, "Retrieving {} block for block_height: {}", finality, block_height);

        let mut block = loop {
            match cache::get_block_and_last_block_height(
                app_state.redis_client.clone(),
                chain_id,
                block_height,
                finality,
            )
            .await?
            {
                (Some(block), _) => break block,
                (_, None) => {
                    return Err(ServiceError::CacheError(
                        "The last block height is missing from the cache".to_string(),
                    ));
                }
                (None, Some(last_block_height)) => {
                    // Not cached
                    if app_state.is_latest {
                        if block_height > last_block_height + MAX_WAIT_BLOCKS {
                            return Ok(HttpResponse::NotFound().json(json!({
                                "error": "The block is too far in the future",
                                "type": "BLOCK_DOES_NOT_EXIST"
                            })));
                        }

                        if block_height > last_block_height {
                            tokio::time::sleep(Duration::from_millis(
                                100 + 1000 * (block_height - last_block_height - 1),
                            ))
                            .await;
                            continue;
                        }

                        if block_height > last_block_height.saturating_sub(EXPECTED_CACHED_BLOCKS) {
                            return Err(ServiceError::CacheError(
                                "The block is not cached".to_string(),
                            ));
                        }
                    }

                    if finality == Finality::Optimistic {
                        // Redirect to the final block
                        return Ok(HttpResponse::Found()
                            .append_header((
                                header::CACHE_CONTROL,
                                format!("public, max-age={}", 24 * 60 * 60),
                            ))
                            .append_header((
                                header::LOCATION,
                                format!("/v0/block/{}", block_height),
                            ))
                            .finish());
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
                    set_multiple_blocks_async(
                        app_state.redis_client.clone(),
                        chain_id,
                        finality,
                        blocks,
                    );
                    break block;
                }
            };
        };

        let mut cache_duration = DEFAULT_CACHE_DURATION;
        if block.is_empty() {
            block = "null".to_string();
            // Temporary avoid caching empty blocks for too long
            cache_duration = Duration::from_secs(24 * 60 * 60);
        }
        Ok(HttpResponse::Ok()
            .append_header((header::CONTENT_TYPE, "application/json; charset=utf-8"))
            .append_header((
                header::CACHE_CONTROL,
                format!("public, max-age={}", cache_duration.as_secs()),
            ))
            .body(block))
    }
}
