use crate::types::*;

const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1000);
const CACHE_EXPIRATION: std::time::Duration = std::time::Duration::from_secs(60);

const TARGET: &str = "cache";

fn block_key(chain_id: ChainId, block_height: BlockHeight) -> String {
    format!("b:{}:{}", chain_id, block_height)
}

pub(crate) async fn get_last_block_height(
    redis_client: redis::Client,
    chain_id: ChainId,
) -> Option<BlockHeight> {
    let mut connection = redis_client
        .get_multiplexed_async_connection_with_timeouts(READ_TIMEOUT, READ_TIMEOUT)
        .await
        .ok()?;
    let key = format!("meta:{}:last_block", chain_id);
    redis::cmd("GET")
        .arg(&key)
        .query_async(&mut connection)
        .await
        .ok()
}

pub(crate) async fn get_block(
    redis_client: redis::Client,
    chain_id: ChainId,
    block_height: BlockHeight,
) -> Option<String> {
    let mut connection = redis_client
        .get_multiplexed_async_connection_with_timeouts(READ_TIMEOUT, READ_TIMEOUT)
        .await
        .ok()?;
    let key = block_key(chain_id, block_height);
    redis::cmd("GET")
        .arg(&key)
        .query_async(&mut connection)
        .await
        .ok()
}

#[allow(dead_code)]
pub(crate) async fn set_block(
    redis_client: redis::Client,
    chain_id: ChainId,
    block_height: BlockHeight,
    block: &str,
) -> Result<(), redis::RedisError> {
    let mut connection = redis_client
        .get_multiplexed_async_connection_with_timeouts(WRITE_TIMEOUT, WRITE_TIMEOUT)
        .await?;
    let key = block_key(chain_id, block_height);
    redis::cmd("SET")
        .arg(&key)
        .arg(block)
        .arg("EX")
        .arg(CACHE_EXPIRATION.as_secs())
        .query_async(&mut connection)
        .await
}

pub(crate) fn set_multiple_blocks_async(
    redis_client: redis::Client,
    chain_id: ChainId,
    blocks: Vec<(BlockHeight, Option<String>)>,
) {
    tokio::spawn((|| async move {
        if let Err(e) = set_multiple_blocks(redis_client, chain_id, blocks).await {
            tracing::warn!(target: TARGET, "Error setting multiple blocks: {:?}", e);
        } else {
            tracing::debug!(target: TARGET, "Successfully set multiple blocks");
        }
    })());
}

async fn set_multiple_blocks(
    redis_client: redis::Client,
    chain_id: ChainId,
    blocks: Vec<(BlockHeight, Option<String>)>,
) -> Result<(), redis::RedisError> {
    let mut connection = redis_client
        .get_multiplexed_async_connection_with_timeouts(WRITE_TIMEOUT, WRITE_TIMEOUT)
        .await?;
    let mut pipe = redis::pipe();
    for (block_height, block) in blocks {
        let key = block_key(chain_id, block_height);
        pipe.cmd("SET")
            .arg(&key)
            .arg(block.unwrap_or_default())
            .arg("EX")
            .arg(CACHE_EXPIRATION.as_secs());
    }
    pipe.query_async(&mut connection).await
}
