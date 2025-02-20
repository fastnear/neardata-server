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
    use crate::reader::archive_filename;

    #[get("/last_block/{finality}")]
    pub async fn get_last_block(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let chain_id = app_state.chain_id;
        let finality =
            Finality::try_from(request.match_info().get("finality").unwrap().to_string())
                .map_err(|_| ServiceError::ArgumentError)?;
        if !app_state.is_fresh {
            // Redirect to the fresh url
            return Ok(HttpResponse::Found()
                .append_header((
                    header::LOCATION,
                    format!(
                        "{}/v0/last_block/{}",
                        app_state.archive_config.as_ref().unwrap().fresh_url,
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
            // Redirect to archive
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

    /// Retrieves a block based on the given block height and finality.
    ///
    /// This function checks if the block height is within valid limits and handles redirects
    /// to archive URLs if necessary. It then attempts to retrieve the block from the cache
    /// or archive, and returns the block data as an HTTP response.
    ///
    /// # Arguments
    ///
    /// * `block_height` - The height of the block to retrieve.
    /// * `finality` - The finality of the block to retrieve (e.g., Final, Optimistic).
    /// * `app_state` - The application state containing configuration and cache information.
    ///
    /// # Returns
    ///
    /// An HTTP response containing the block data or an error message.
    async fn get_block_inner(
        block_height: BlockHeight,
        finality: Finality,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let chain_id = app_state.chain_id.clone();

        // Check if the block height is within valid limits
        if let Some(response) = check_block_height_limits(block_height, &app_state) {
            return Ok(response);
        }

        // Handle redirects to archive URLs if necessary
        if let Some(response) = check_archive_redirects(block_height, finality, &app_state) {
            return Ok(response);
        }

        tracing::debug!(target: TARGET_API, "Retrieving {} block for block_height: {}", finality, block_height);

        // Retrieve the block from the cache or archive
        let block =
            retrieve_block_from_cache_or_archive(block_height, finality, &app_state, chain_id)
                .await?;

        // Determine the cache duration based on whether the block is empty
        let cache_duration = if block.is_empty() {
            Duration::from_secs(24 * 60 * 60)
        } else {
            DEFAULT_CACHE_DURATION
        };

        // Return the block data as an HTTP response
        Ok(HttpResponse::Ok()
            .append_header((header::CONTENT_TYPE, "application/json; charset=utf-8"))
            .append_header((
                header::CACHE_CONTROL,
                format!("public, max-age={}", cache_duration.as_secs()),
            ))
            .body(block))
    }

    /// Checks if the block height is within valid limits.
    ///
    /// # Arguments
    ///
    /// * `block_height` - The height of the block to check.
    /// * `app_state` - The application state containing configuration information.
    ///
    /// # Returns
    ///
    /// An optional HTTP response indicating an error if the block height is out of bounds.
    fn check_block_height_limits(
        block_height: BlockHeight,
        app_state: &web::Data<AppState>,
    ) -> Option<HttpResponse> {
        if block_height > MAX_BLOCK_HEIGHT {
            return Some(
                HttpResponse::NotFound()
                    .append_header((
                        header::CACHE_CONTROL,
                        format!("public, max-age={}", 24 * 60 * 60),
                    ))
                    .json(json!({
                        "error": "Block height is too high",
                        "type": "BLOCK_HEIGHT_TOO_HIGH"
                    })),
            );
        }
        if block_height < app_state.genesis_block_height {
            return Some(
                HttpResponse::NotFound()
                    .append_header((
                        header::CACHE_CONTROL,
                        format!("public, max-age={}", 24 * 60 * 60),
                    ))
                    .json(json!({
                        "error": "Block height is before the genesis",
                        "type": "BLOCK_HEIGHT_TOO_LOW"
                    })),
            );
        }
        None
    }

    /// Handles redirects to archive URLs if necessary.
    ///
    /// # Arguments
    ///
    /// * `block_height` - The height of the block to check.
    /// * `finality` - The finality of the block to check.
    /// * `app_state` - The application state containing configuration information.
    ///
    /// # Returns
    ///
    /// An optional HTTP response indicating a redirect to an archive URL.
    fn check_archive_redirects(
        block_height: BlockHeight,
        finality: Finality,
        app_state: &web::Data<AppState>,
    ) -> Option<HttpResponse> {
        if let Some(archive_config) = &app_state.archive_config {
            if app_state.is_latest && block_height < archive_config.end_height {
                return Some(
                    HttpResponse::Found()
                        .append_header((
                            header::CACHE_CONTROL,
                            format!("public, max-age={}", 24 * 60 * 60),
                        ))
                        .append_header((
                            header::LOCATION,
                            format!("{}/v0/block/{}", archive_config.archive_url, block_height),
                        ))
                        .finish(),
                );
            } else if !app_state.is_latest
                && (block_height >= archive_config.end_height || finality == Finality::Optimistic)
            {
                return Some(
                    HttpResponse::Found()
                        .append_header((
                            header::CACHE_CONTROL,
                            format!("public, max-age={}", 24 * 60 * 60),
                        ))
                        .append_header((
                            header::LOCATION,
                            format!(
                                "{}/v0/block{}/{}",
                                archive_config.fresh_url,
                                finality_suffix(finality),
                                block_height
                            ),
                        ))
                        .finish(),
                );
            }
        }
        None
    }

    /// Retrieves the block from the cache or archive.
    ///
    /// # Arguments
    ///
    /// * `block_height` - The height of the block to retrieve.
    /// * `finality` - The finality of the block to retrieve.
    /// * `app_state` - The application state containing configuration and cache information.
    /// * `chain_id` - The chain ID of the blockchain.
    ///
    /// # Returns
    ///
    /// The block data as a string or an error.
    async fn retrieve_block_from_cache_or_archive(
        block_height: BlockHeight,
        finality: Finality,
        app_state: &web::Data<AppState>,
        chain_id: ChainId,
    ) -> Result<String, ServiceError> {
        loop {
            match cache::get_block_and_last_block_height(
                app_state.redis_client.clone(),
                chain_id.clone(),
                block_height,
                finality,
            )
            .await?
            {
                (Some(block), _) => return Ok(block),
                (_, None) => {
                    return Err(ServiceError::CacheError(
                        "The last block height is missing from the cache".to_string(),
                    ));
                }
                (None, Some(last_block_height)) => {
                    if let Some(block) = handle_not_cached_block(
                        block_height,
                        last_block_height,
                        finality,
                        &app_state,
                        chain_id.clone(),
                    )
                    .await?
                    {
                        return Ok(block);
                    }
                }
            }
        }
    }

    /// Handles the case where the block is not cached.
    ///
    /// # Arguments
    ///
    /// * `block_height` - The height of the block to retrieve.
    /// * `last_block_height` - The height of the last block in the cache.
    /// * `finality` - The finality of the block to retrieve.
    /// * `app_state` - The application state containing configuration and cache information.
    /// * `chain_id` - The chain ID of the blockchain.
    ///
    /// # Returns
    ///
    /// An optional block data as a string or an error.
    async fn handle_not_cached_block(
        block_height: BlockHeight,
        last_block_height: BlockHeight,
        finality: Finality,
        app_state: &web::Data<AppState>,
        chain_id: ChainId,
    ) -> Result<Option<String>, ServiceError> {
        if app_state.is_latest {
            if block_height > last_block_height + MAX_WAIT_BLOCKS {
                return Ok(Some(
                    json!({
                        "error": "The block is too far in the future",
                        "type": "BLOCK_DOES_NOT_EXIST"
                    })
                    .to_string(),
                ));
            }

            if block_height > last_block_height {
                cache::wait_for_block(
                    app_state.redis_client.clone(),
                    chain_id,
                    block_height,
                    finality,
                    Duration::from_millis(1000 * (block_height - last_block_height + 1)),
                )
                .await?;
                return Ok(None);
            }

            if block_height > last_block_height.saturating_sub(EXPECTED_CACHED_BLOCKS) {
                return Err(ServiceError::CacheError(
                    "The block is not cached".to_string(),
                ));
            }
        }

        if finality == Finality::Optimistic {
            return Ok(Some(
                json!({
                    "error": "The block is not cached",
                    "type": "BLOCK_NOT_CACHED"
                })
                .to_string(),
            ));
        }

        if app_state.read_config.is_none() {
            return Ok(Some(
                json!({
                    "error": "The block is not cached and no read config is available",
                    "type": "BLOCK_NOT_CACHED_NO_READ_CONFIG"
                })
                .to_string(),
            ));
        }

        let archive_fn = archive_filename(
            &app_state.read_config.as_ref().unwrap(),
            chain_id,
            block_height,
        );
        let should_read =
            cache::acquire_archive_read_attempt(app_state.redis_client.clone(), &archive_fn)
                .await?;

        if !should_read {
            tokio::time::sleep(Duration::from_millis(100)).await;
            return Ok(None);
        }

        let blocks = read_blocks(
            &app_state.read_config.as_ref().unwrap(),
            chain_id,
            block_height,
        );
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
        set_multiple_blocks_async(app_state.redis_client.clone(), chain_id, finality, blocks);
        Ok(Some(block))
    }
}
