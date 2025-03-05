use crate::cache::set_multiple_blocks_async;
use crate::reader::read_blocks;
use crate::types::*;
use crate::*;
use actix_web::ResponseError;
use reqwest::header::HeaderName;
use serde_json::json;
use std::fmt;
use std::str::FromStr;
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
    InternalDataError,
}

#[derive(Debug)]
enum BlockOrResponse {
    Block(String),
    Response(HttpResponse),
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
            ServiceError::InternalDataError => write!(f, "Internal data error"),
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
            ServiceError::InternalDataError => {
                HttpResponse::InternalServerError().json("Internal data error")
            }
        }
    }
}

fn arg<T: FromStr>(request: &HttpRequest, name: &str) -> Result<T, ServiceError> {
    request
        .match_info()
        .get(name)
        .unwrap()
        .parse::<T>()
        .map_err(|_| ServiceError::ArgumentError)
}

fn arg_finality(request: &HttpRequest) -> Finality {
    if request.match_info().get("finality") == Some("_opt") {
        Finality::Optimistic
    } else {
        Finality::Final
    }
}

fn header(http_response: &HttpResponse, name: HeaderName) -> Option<String> {
    Some(
        http_response
            .headers()
            .get(name)?
            .to_str()
            .ok()?
            .to_string(),
    )
}

pub mod v0 {
    use super::*;
    use crate::cache::finality_suffix;
    use crate::reader::archive_filename;
    use actix_web::body::MessageBody;
    use actix_web::http::header::HeaderValue;
    use reqwest::StatusCode;
    use serde_json::Value;

    #[get("/last_block/{finality}{suffix:/?.*}")]
    pub async fn get_last_block(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let chain_id = app_state.chain_id;
        let finality =
            Finality::try_from(request.match_info().get("finality").unwrap().to_string())
                .map_err(|_| ServiceError::ArgumentError)?;
        let suffix = request.match_info().get("suffix").unwrap_or_default();
        if !app_state.is_fresh {
            // Redirect to the fresh url
            return Ok(HttpResponse::Found()
                .append_header((
                    header::LOCATION,
                    format!(
                        "https://{}/v0/last_block/{}{}",
                        app_state.archive_config.as_ref().unwrap().domain_name,
                        finality,
                        suffix
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
                    "/v0/block{}/{}{}",
                    finality_suffix(finality),
                    last_block_height,
                    suffix
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
            if archive_config.archive_index != 0 {
                return Ok(HttpResponse::Found()
                    .append_header((
                        header::CACHE_CONTROL,
                        format!("public, max-age={}", 24 * 60 * 60),
                    ))
                    .append_header((
                        header::LOCATION,
                        format!(
                            "https://a0.{}/v0/block/{}",
                            archive_config.domain_name, app_state.genesis_block_height
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

    #[get("/block{finality:(_opt)?}/{block_height}")]
    pub async fn get_block(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let finality = arg_finality(&request);
        let block_height: BlockHeight = arg(&request, "block_height")?;
        get_block_inner(block_height, finality, app_state).await
    }

    #[get("/block{finality:(_opt)?}/{block_height}/headers")]
    pub async fn get_block_headers(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let finality = arg_finality(&request);
        let block_height: BlockHeight = arg(&request, "block_height")?;
        let response = get_block_inner(block_height, finality, app_state.clone()).await?;

        redirect_or_map(response, "/headers", |block_json| {
            Ok(block_json.get("block").cloned().unwrap_or(Value::Null))
        })
    }

    #[get("/block{finality:(_opt)?}/{block_height}/chunk/{shard_id}")]
    pub async fn get_chunk(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let finality = arg_finality(&request);
        let block_height: BlockHeight = arg(&request, "block_height")?;
        let shard_id: u64 = arg(&request, "shard_id")?;

        let response = get_block_inner(block_height, finality, app_state.clone()).await?;

        redirect_or_map(response, &format!("/chunk/{shard_id}"), move |block_json| {
            Ok(block_json
                .get("shards")
                .and_then(|shards| shards.as_array())
                .and_then(|shards| {
                    shards
                        .iter()
                        .find(|shard| shard["shard_id"].as_u64() == Some(shard_id))
                        .and_then(|shard| shard.get("chunk"))
                })
                .cloned()
                .unwrap_or(Value::Null))
        })
    }

    #[get("/block{finality:(_opt)?}/{block_height}/shard/{shard_id}")]
    pub async fn get_shard(
        request: HttpRequest,
        app_state: web::Data<AppState>,
    ) -> Result<impl Responder, ServiceError> {
        let finality = arg_finality(&request);
        let block_height: BlockHeight = arg(&request, "block_height")?;
        let shard_id: u64 = arg(&request, "shard_id")?;

        let response = get_block_inner(block_height, finality, app_state.clone()).await?;

        redirect_or_map(response, &format!("/shard/{shard_id}"), move |block_json| {
            Ok(block_json
                .get("shards")
                .and_then(|shards| shards.as_array())
                .and_then(|shards| {
                    shards
                        .iter()
                        .find(|shard| shard["shard_id"].as_u64() == Some(shard_id))
                })
                .cloned()
                .unwrap_or(Value::Null))
        })
    }

    fn redirect_or_map<F>(
        mut response: HttpResponse,
        suffix: &str,
        f: F,
    ) -> Result<impl Responder, ServiceError>
    where
        F: FnOnce(Value) -> Result<Value, ServiceError>,
    {
        match response.status() {
            StatusCode::FOUND => {
                let previous_location = header(&response, header::LOCATION).unwrap();

                response.headers_mut().insert(
                    header::LOCATION,
                    HeaderValue::from_str(&format!("{}{}", previous_location, suffix)).unwrap(),
                );
                Ok(response)
            }
            StatusCode::OK => {
                // We need to grab the CACHE_CONTROL header from the response and return it
                let cache_control_header = header(&response, header::CACHE_CONTROL).unwrap();

                let body_bytes = response.into_body().try_into_bytes().unwrap();
                let block_json: Value = serde_json::from_slice(&body_bytes)
                    .map_err(|_| ServiceError::InternalDataError)?;
                f(block_json).and_then(|block_json| {
                    Ok(HttpResponse::Ok()
                        .insert_header((header::CACHE_CONTROL, cache_control_header))
                        .json(block_json))
                })
            }
            _ => Ok(response),
        }
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
    ) -> Result<HttpResponse, ServiceError> {
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
        let block_or_response =
            retrieve_block_from_cache_or_archive(block_height, finality, &app_state, chain_id)
                .await?;

        let mut block = match block_or_response {
            BlockOrResponse::Block(block) => block,
            BlockOrResponse::Response(response) => return Ok(response),
        };

        // Determine the cache duration based on whether the block is empty
        let cache_duration = if block.is_empty() {
            block = "null".to_string();
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
            if !app_state.is_fresh && finality == Finality::Optimistic {
                // Redirect to the fresh server
                return Some(
                    HttpResponse::Found()
                        .append_header((
                            header::CACHE_CONTROL,
                            format!("public, max-age={}", 24 * 60 * 60),
                        ))
                        .append_header((
                            header::LOCATION,
                            format!(
                                "https://{}/v0/block{}/{}",
                                archive_config.domain_name,
                                finality_suffix(finality),
                                block_height
                            ),
                        ))
                        .finish(),
                );
            }
            // Find the required archive index
            let index = archive_config
                .archive_boundaries
                .iter()
                .position(|&x| block_height < x)
                .unwrap_or(archive_config.archive_boundaries.len());
            if index != archive_config.archive_index {
                return Some(
                    HttpResponse::Found()
                        .append_header((
                            header::CACHE_CONTROL,
                            format!("public, max-age={}", 24 * 60 * 60),
                        ))
                        .append_header((
                            header::LOCATION,
                            format!(
                                "https://a{}.{}/v0/block/{}",
                                index, archive_config.domain_name, block_height
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
    ) -> Result<BlockOrResponse, ServiceError> {
        loop {
            match cache::get_block_and_last_block_height(
                app_state.redis_client.clone(),
                chain_id.clone(),
                block_height,
                finality,
            )
            .await?
            {
                (Some(block), _) => return Ok(BlockOrResponse::Block(block)),
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
    ) -> Result<Option<BlockOrResponse>, ServiceError> {
        if app_state.is_latest {
            if block_height > last_block_height + MAX_WAIT_BLOCKS {
                return Ok(Some(BlockOrResponse::Response(
                    HttpResponse::NotFound().json(json!({
                        "error": "The block is too far in the future",
                        "type": "BLOCK_DOES_NOT_EXIST"
                    })),
                )));
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
            return Ok(Some(BlockOrResponse::Response(
                HttpResponse::Found()
                    .append_header((
                        header::CACHE_CONTROL,
                        format!("public, max-age={}", 24 * 60 * 60),
                    ))
                    .append_header((header::LOCATION, format!("/v0/block/{}", block_height)))
                    .finish(),
            )));
        }

        // If the read-path is not set, it means the server doesn't use archive files.
        // We have to redirect to the latest server with files.
        if app_state.read_config.is_none() {
            let archive_config = app_state
                .archive_config
                .as_ref()
                .expect("Missing archive config without local files config");
            return Ok(Some(BlockOrResponse::Response(
                HttpResponse::Found()
                    .append_header((
                        header::CACHE_CONTROL,
                        format!("public, max-age={}", 24 * 60 * 60),
                    ))
                    .append_header((
                        header::LOCATION,
                        format!(
                            "https://a{}.{}/v0/block/{}",
                            archive_config.archive_boundaries.len(),
                            archive_config.domain_name,
                            block_height
                        ),
                    ))
                    .finish(),
            )));
        }

        // Before reading blocks we'll check the last time the archive was accessed and
        // indicate we want to read it.
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
        Ok(Some(BlockOrResponse::Block(block)))
    }
}

#[get("/health")]
pub async fn health(app_state: web::Data<AppState>) -> Result<impl Responder, ServiceError> {
    if !app_state.is_latest {
        return Ok(HttpResponse::Ok().json(json!({"status": "ok"})));
    }
    let chain_id = app_state.chain_id;
    let finality = Finality::Final;
    let block_height =
        cache::get_last_block_height(app_state.redis_client.clone(), chain_id, finality)
            .await
            .ok_or_else(|| {
                ServiceError::CacheError(
                    "The last block height is missing from the cache".to_string(),
                )
            })?;
    match cache::get_block_and_last_block_height(
        app_state.redis_client.clone(),
        chain_id,
        block_height,
        finality,
    )
    .await?
    {
        (Some(block), _) => {
            let block: serde_json::Value = serde_json::from_str(&block)
                .map_err(|_| ServiceError::CacheError("Failed to parse the block".to_string()))?;
            let timestamp = block["block"]["header"]["timestamp_nanosec"]
                .as_str()
                .ok_or_else(|| {
                    ServiceError::CacheError("The block is missing a timestamp".to_string())
                })?;
            let t_nano = timestamp.parse::<u128>().unwrap_or(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default();
            let sync_latency_ms = now.as_nanos().saturating_sub(t_nano) / 1_000_000;
            if sync_latency_ms > app_state.max_healthy_latency_ms {
                return Ok(HttpResponse::Ok().json(json!({"status": "unhealthy"})));
            }
        }
        _ => {
            return Err(ServiceError::CacheError(
                "The block is not cached".to_string(),
            ));
        }
    }

    Ok(HttpResponse::Ok().json(json!({"status": "ok"})))
}
