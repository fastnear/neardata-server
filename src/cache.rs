use crate::metrics;
use crate::types::*;
use crate::with_retries;

const REDIS_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(5000);
const CACHE_EXPIRATION: std::time::Duration = std::time::Duration::from_secs(60);
const ARCHIVE_ATTEMPT_CACHE_EXPIRATION: std::time::Duration =
    CACHE_EXPIRATION.saturating_sub(std::time::Duration::from_secs(5));

/// How many leading bytes of a block JSON we fetch to find the header timestamp.
/// The block header's `timestamp_nanosec` sits at byte ~820-840 in every block we
/// have on hand, so this leaves more than 2x headroom while transferring ~2 KiB
/// instead of the ~750 KiB-1 MiB of a whole block.
const TIP_PREFIX_BYTES: isize = 2047;

/// The block header is the only place this key appears in a block document, so a
/// plain substring search is unambiguous. It has to include the opening quote of
/// the value: the integer `"timestamp"` field sits immediately before it.
const TIMESTAMP_NEEDLE: &str = "\"timestamp_nanosec\":\"";

const TARGET: &str = "cache";

pub(crate) fn finality_suffix(finality: Finality) -> &'static str {
    match finality {
        Finality::Final => "",
        Finality::Optimistic => "_opt",
    }
}

fn block_key(chain_id: ChainId, block_height: BlockHeight, finality: Finality) -> String {
    format!(
        "b:{}{}:{}",
        chain_id,
        finality_suffix(finality),
        block_height
    )
}

fn last_block_key(chain_id: ChainId, finality: Finality) -> String {
    format!("meta:{}{}:last_block", chain_id, finality_suffix(finality))
}

/// The latest block height for a given finality together with the block's own
/// header timestamp, i.e. everything needed to say how far behind the chain we are.
#[derive(Debug, Clone, Copy)]
pub struct TipObservation {
    pub height: BlockHeight,
    pub timestamp_nanos: u128,
    /// True when the cheap `GETRANGE` prefix didn't contain the timestamp and we
    /// had to fall back to fetching the whole block.
    pub used_full_fetch: bool,
}

#[derive(Debug)]
pub enum TipError {
    /// `meta:{chain}{suffix}:last_block` is missing from the cache.
    TipMissing,
    /// The tip height is known, but its block isn't cached (or is cached empty).
    BlockMissing,
    /// The block is cached but carries no parsable `timestamp_nanosec`.
    ParseError,
    Redis(redis::RedisError),
}

impl TipError {
    /// Stable label value for `neardata_tip_poll_total{result}`.
    pub fn result_label(&self) -> &'static str {
        match self {
            TipError::TipMissing => "tip_missing",
            TipError::BlockMissing => "block_missing",
            TipError::ParseError => "parse_error",
            TipError::Redis(_) => "redis_error",
        }
    }
}

/// Last resort behind [`parse_timestamp_nanos`]: a real JSON parse, so a block
/// serialized with different whitespace still yields a timestamp instead of
/// silently blinding the tip metrics. Costs a full parse of a ~1 MiB document,
/// which is why it is never on the normal path.
fn parse_timestamp_nanos_json(block_json: &str) -> Option<u128> {
    let block: serde_json::Value = serde_json::from_str(block_json).ok()?;
    block["block"]["header"]["timestamp_nanosec"]
        .as_str()?
        .parse()
        .ok()
}

pub(crate) fn parse_timestamp_nanos(block_json: &str) -> Option<u128> {
    let start = block_json.find(TIMESTAMP_NEEDLE)? + TIMESTAMP_NEEDLE.len();
    let rest = &block_json[start..];
    // The closing quote has to be there. Without this check a prefix that cut the
    // number in half would parse into a plausible-looking timestamp from 1970 and
    // report a latency of half a century.
    let end = rest.find('"')?;
    rest[..end].parse().ok()
}

pub fn latency_ms_from_nanos(timestamp_nanos: u128) -> u128 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    now.as_nanos().saturating_sub(timestamp_nanos) / 1_000_000
}

/// Reads the tip height and its block timestamp on a caller-owned connection.
///
/// Used by the metrics poller, which holds a connection across ticks and treats
/// the poll loop itself as the retry, so it deliberately does not go through
/// [`with_retries`].
pub(crate) async fn tip_observation_on(
    connection: &mut redis::aio::MultiplexedConnection,
    chain_id: ChainId,
    finality: Finality,
) -> Result<TipObservation, TipError> {
    let height: Option<BlockHeight> = redis::cmd("GET")
        .arg(last_block_key(chain_id, finality))
        .query_async(connection)
        .await
        .map_err(TipError::Redis)?;
    let height = height.ok_or(TipError::TipMissing)?;

    let key = block_key(chain_id, height, finality);
    // Read raw bytes, not a String: a byte range can cut a multi-byte UTF-8
    // character in half, and redis-rs would reject that as a type error which we
    // would then misreport as a redis failure. The needle and the digits are
    // ASCII, so lossily decoding a mangled tail costs us nothing.
    let prefix: Vec<u8> = redis::cmd("GETRANGE")
        .arg(&key)
        .arg(0)
        .arg(TIP_PREFIX_BYTES)
        .query_async(connection)
        .await
        .map_err(TipError::Redis)?;
    if prefix.is_empty() {
        return Err(TipError::BlockMissing);
    }
    let prefix = String::from_utf8_lossy(&prefix);
    if let Some(timestamp_nanos) = parse_timestamp_nanos(&prefix) {
        return Ok(TipObservation {
            height,
            timestamp_nanos,
            used_full_fetch: false,
        });
    }

    // The prefix should always contain the header timestamp. If a future block
    // format moves it, degrade to fetching the whole block rather than going blind.
    tracing::warn!(
        target: TARGET,
        "The {} block {} has no timestamp in its first {} bytes, falling back to a full fetch",
        finality,
        height,
        TIP_PREFIX_BYTES + 1
    );
    let block: Option<String> = redis::cmd("GET")
        .arg(&key)
        .query_async(connection)
        .await
        .map_err(TipError::Redis)?;
    let block = block
        .filter(|b| !b.is_empty())
        .ok_or(TipError::BlockMissing)?;
    let timestamp_nanos = parse_timestamp_nanos(&block)
        .or_else(|| parse_timestamp_nanos_json(&block))
        .ok_or(TipError::ParseError)?;
    Ok(TipObservation {
        height,
        timestamp_nanos,
        used_full_fetch: true,
    })
}

/// Retrying wrapper around [`tip_observation_on`] for the request path.
pub(crate) async fn get_tip_observation(
    redis_client: redis::Client,
    chain_id: ChainId,
    finality: Finality,
) -> Result<TipObservation, TipError> {
    // `with_retries!` only knows how to retry redis errors, so non-redis outcomes
    // ride out through the Ok branch instead of burning the retry budget.
    let res: redis::RedisResult<Result<TipObservation, TipError>> = with_retries!(
        redis_client,
        metrics::OP_TIP_OBSERVATION,
        |connection| async {
            match tip_observation_on(connection, chain_id, finality).await {
                Err(TipError::Redis(err)) => Err(err),
                other => Ok(other),
            }
        }
    );
    res.map_err(TipError::Redis)?
}

pub(crate) async fn get_last_block_height(
    redis_client: redis::Client,
    chain_id: ChainId,
    finality: Finality,
) -> Option<BlockHeight> {
    let res: redis::RedisResult<BlockHeight> = with_retries!(
        redis_client,
        metrics::OP_GET_LAST_BLOCK,
        |connection| async {
            let key = last_block_key(chain_id, finality);
            redis::cmd("GET").arg(&key).query_async(connection).await
        }
    );
    res.ok()
}

pub(crate) async fn get_block_and_last_block_height(
    redis_client: redis::Client,
    chain_id: ChainId,
    block_height: BlockHeight,
    finality: Finality,
) -> redis::RedisResult<(Option<String>, Option<BlockHeight>)> {
    let res: redis::RedisResult<(Option<String>, Option<String>)> = with_retries!(
        redis_client,
        metrics::OP_GET_BLOCK_AND_LAST_BLOCK,
        |connection| async {
            redis::pipe()
                .cmd("GET")
                .arg(block_key(chain_id, block_height, finality))
                .cmd("GET")
                .arg(last_block_key(chain_id, finality))
                .query_async(connection)
                .await
        }
    );
    let res = res?;

    Ok((res.0, res.1.map(|s| s.parse().unwrap())))
}

#[allow(dead_code)]
pub(crate) async fn set_block(
    redis_client: redis::Client,
    chain_id: ChainId,
    block_height: BlockHeight,
    finality: Finality,
    block: &str,
) -> Result<(), redis::RedisError> {
    with_retries!(redis_client, metrics::OP_SET_BLOCK, |connection| async {
        let key = block_key(chain_id, block_height, finality);
        redis::cmd("SET")
            .arg(&key)
            .arg(block)
            .arg("EX")
            .arg(CACHE_EXPIRATION.as_secs())
            .query_async(connection)
            .await
    })
}

pub(crate) async fn acquire_archive_read_attempt(
    redis_client: redis::Client,
    archive_path: &str,
) -> Result<bool, redis::RedisError> {
    with_retries!(
        redis_client,
        metrics::OP_ACQUIRE_ARCHIVE_LOCK,
        |connection| async {
            let key = format!("archive_read_attempt:{}", archive_path);
            redis::cmd("SET")
                .arg(&key)
                .arg("1")
                .arg("NX")
                .arg("EX")
                .arg(ARCHIVE_ATTEMPT_CACHE_EXPIRATION.as_secs())
                .query_async(connection)
                .await
        }
    )
    .map(|res: Option<String>| res.is_some())
}

pub(crate) async fn wait_for_block(
    redis_client: redis::Client,
    chain_id: ChainId,
    block_height: BlockHeight,
    finality: Finality,
    max_timeout: std::time::Duration,
) -> redis::RedisResult<()> {
    let id = format!("{}-0", block_height - 1);
    let key = format!(
        "meta:{}{}:last_blocks_queue",
        chain_id,
        finality_suffix(finality)
    );
    let _res: redis::Value = with_retries!(
        redis_client,
        metrics::OP_WAIT_FOR_BLOCK,
        |connection| async {
            redis::cmd("XREAD")
                .arg("BLOCK")
                .arg(max_timeout.as_millis() as u64)
                .arg("STREAMS")
                .arg(&key)
                .arg(&id)
                .query_async(connection)
                .await
        }
    )?;
    Ok(())
}

pub(crate) fn set_multiple_blocks_async(
    redis_client: redis::Client,
    chain_id: ChainId,
    finality: Finality,
    blocks: Vec<(BlockHeight, Option<String>)>,
) {
    tokio::spawn((|| async move {
        let count = blocks.len() as u64;
        if let Err(e) = set_multiple_blocks(redis_client, chain_id, finality, blocks).await {
            metrics::record_cache_block_writes(finality, false, count);
            tracing::warn!(target: TARGET, "Error setting multiple blocks: {:?}", e);
        } else {
            metrics::record_cache_block_writes(finality, true, count);
            tracing::debug!(target: TARGET, "Successfully set multiple blocks");
        }
    })());
}

async fn set_multiple_blocks(
    redis_client: redis::Client,
    chain_id: ChainId,
    finality: Finality,
    blocks: Vec<(BlockHeight, Option<String>)>,
) -> Result<(), redis::RedisError> {
    with_retries!(
        redis_client,
        metrics::OP_SET_MULTIPLE_BLOCKS,
        |connection| async {
            let mut pipe = redis::pipe();
            for (block_height, block) in &blocks {
                let key = block_key(chain_id, *block_height, finality);
                pipe.cmd("SET")
                    .arg(&key)
                    .arg(block.as_ref().map(|s| s.as_str()).unwrap_or_default())
                    .arg("EX")
                    .arg(CACHE_EXPIRATION.as_secs());
            }
            pipe.query_async(connection).await
        }
    )
}

/// Retries a redis call, recording its duration, outcome and retry count under
/// the given logical `op` label.
///
/// The two-argument form is kept so a call site that hasn't been given an op yet
/// still compiles; it reports as `op="unknown"`.
#[macro_export]
macro_rules! with_retries {
    ($client: expr, $f_async: expr) => {
        $crate::with_retries!($client, "unknown", $f_async)
    };
    ($client: expr, $op: expr, $f_async: expr) => {
        {
            let __op: &'static str = $op;
            let __started = std::time::Instant::now();
            let mut delay = tokio::time::Duration::from_millis(100);
            let max_retries = 7;
            let mut i = 0;
            let __result = loop {
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
            };
            // `i` is the number of attempts that failed, which is the retry
            // count only when one of them eventually succeeded.
            $crate::metrics::record_redis_op(
                __op,
                __started.elapsed().as_secs_f64(),
                __result.is_ok(),
                i as u64,
            );
            __result
        }
    };
}

#[cfg(test)]
mod tests {
    use super::parse_timestamp_nanos;
    use crate::reader::read_blocks;
    use crate::types::ChainId;
    use crate::ReadConfig;

    /// The real mainnet block checked into `res/`, so this test breaks if the
    /// block format ever moves the header timestamp.
    fn sample_block() -> String {
        let config = ReadConfig {
            path: "res".to_string(),
            save_every_n: 10,
        };
        read_blocks(&config, ChainId::Mainnet, 131594520)
            .into_iter()
            .find_map(|(height, block)| (height == 131594520).then_some(block))
            .expect("the sample archive should contain block 131594520")
            .expect("block 131594520 should not be empty")
    }

    #[test]
    fn parses_the_header_timestamp_from_a_real_block() {
        assert_eq!(
            parse_timestamp_nanos(&sample_block()),
            Some(1730339327218714330)
        );
    }

    #[test]
    fn header_timestamp_is_within_the_fetched_prefix() {
        let block = sample_block();
        // Cut on bytes and decode lossily, exactly as GETRANGE plus
        // tip_observation_on do, so the test cannot pass by slicing more
        // conveniently than production does.
        let prefix =
            String::from_utf8_lossy(&block.as_bytes()[..super::TIP_PREFIX_BYTES as usize + 1])
                .into_owned();
        assert_eq!(
            parse_timestamp_nanos(&prefix),
            Some(1730339327218714330),
            "the GETRANGE prefix must be long enough to contain the header timestamp"
        );
    }

    #[test]
    fn survives_a_prefix_that_cuts_a_utf8_character_in_half() {
        // GETRANGE cuts on bytes, so the fetched prefix can end mid-character.
        // The block itself is fine; only our window into it is ragged.
        let mut bytes = b"{\"block\":{\"header\":{\"timestamp_nanosec\":\"1730339327218714330\",\"pad\":\"\xc3\xa9".to_vec();
        bytes.pop(); // drop the second byte of the two-byte character
        assert!(
            std::str::from_utf8(&bytes).is_err(),
            "the fixture must be invalid UTF-8"
        );
        assert_eq!(
            parse_timestamp_nanos(&String::from_utf8_lossy(&bytes)),
            Some(1730339327218714330)
        );
    }

    #[test]
    fn json_fallback_handles_a_reformatted_block() {
        // The substring scan expects compact serde output. If a block ever comes
        // back pretty-printed the fallback still finds the timestamp.
        let pretty = "{\n  \"block\": {\n    \"header\": {\n      \"timestamp_nanosec\": \"1730339327218714330\"\n    }\n  }\n}";
        assert_eq!(parse_timestamp_nanos(pretty), None);
        assert_eq!(
            super::parse_timestamp_nanos_json(pretty),
            Some(1730339327218714330)
        );
    }

    #[test]
    fn returns_none_when_the_timestamp_is_absent_or_truncated() {
        assert_eq!(parse_timestamp_nanos(""), None);
        assert_eq!(parse_timestamp_nanos("{\"block\":{\"header\":{}}}"), None);
        // Needle present but the value is cut off mid-number by the prefix boundary.
        assert_eq!(
            parse_timestamp_nanos("{\"timestamp_nanosec\":\"17303393"),
            None
        );
        // The integer `timestamp` field alone must not match.
        assert_eq!(
            parse_timestamp_nanos("{\"timestamp\":1730339327218714330}"),
            None
        );
    }
}
