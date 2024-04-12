use crate::types::*;
use crate::with_retries;

const REDIS_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(5000);
const CACHE_EXPIRATION: std::time::Duration = std::time::Duration::from_secs(60);

const TARGET: &str = "cache";

fn block_key(chain_id: ChainId, block_height: BlockHeight) -> String {
    format!("b:{}:{}", chain_id, block_height)
}

pub(crate) async fn get_last_block_height(
    redis_client: redis::Client,
    chain_id: ChainId,
) -> Option<BlockHeight> {
    let res: redis::RedisResult<BlockHeight> = with_retries!(redis_client, |connection| async {
        let key = format!("meta:{}:last_block", chain_id);
        redis::cmd("GET").arg(&key).query_async(connection).await
    });
    res.ok()
}

pub(crate) async fn get_block_and_last_block_height(
    redis_client: redis::Client,
    chain_id: ChainId,
    block_height: BlockHeight,
) -> redis::RedisResult<(Option<String>, Option<BlockHeight>)> {
    let res: redis::RedisResult<(Option<String>, Option<String>)> =
        with_retries!(redis_client, |connection| async {
            redis::pipe()
                .cmd("GET")
                .arg(block_key(chain_id, block_height))
                .cmd("GET")
                .arg(format!("meta:{}:last_block", chain_id))
                .query_async(connection)
                .await
        });
    let res = res?;

    Ok((res.0, res.1.map(|s| s.parse().unwrap())))
}

#[allow(dead_code)]
pub(crate) async fn set_block(
    redis_client: redis::Client,
    chain_id: ChainId,
    block_height: BlockHeight,
    block: &str,
) -> Result<(), redis::RedisError> {
    with_retries!(redis_client, |connection| async {
        let key = block_key(chain_id, block_height);
        redis::cmd("SET")
            .arg(&key)
            .arg(block)
            .arg("EX")
            .arg(CACHE_EXPIRATION.as_secs())
            .query_async(connection)
            .await
    })
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
    with_retries!(redis_client, |connection| async {
        let mut pipe = redis::pipe();
        for (block_height, block) in &blocks {
            let key = block_key(chain_id, *block_height);
            pipe.cmd("SET")
                .arg(&key)
                .arg(block.as_ref().map(|s| s.as_str()).unwrap_or_default())
                .arg("EX")
                .arg(CACHE_EXPIRATION.as_secs());
        }
        pipe.query_async(connection).await
    })
}

#[macro_export]
macro_rules! with_retries {
    ($client: expr, $f_async: expr) => {
        {
            let mut delay = tokio::time::Duration::from_millis(100);
            let max_retries = 7;
            let mut i = 0;
            loop {
                let connection =
                    $client.get_multiplexed_async_connection_with_timeouts(REDIS_TIMEOUT, REDIS_TIMEOUT)
                    .await;
                let err = match connection {
                    Ok(mut connection) => {
                        match $f_async(&mut connection).await {
                            Ok(v) => break Ok(v),
                            Err(err) => err,
                        }
                    }
                    Err(err) => err,
                };
                tracing::log::error!(target: "redis", "Attempt #{}: connection error {}", i, err);
                tokio::time::sleep(delay).await;
                delay *= 2;
                i += 1;
                if i >= max_retries {
                    break Err(err);
                }
            }
        }
    };
}
