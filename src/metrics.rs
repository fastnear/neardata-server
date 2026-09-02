//! Prometheus instrumentation and the `/metrics` endpoint.
//!
//! Every metric lives in the `prometheus` crate's process-global default
//! registry. That matters here: `AppState` is rebuilt inside the
//! `HttpServer::new` factory closure, so it exists once *per worker thread* and
//! is the wrong place to keep counters. The global registry is shared across
//! workers, so increments from any thread aggregate correctly and a scrape from
//! any thread sees all of them.

use std::sync::LazyLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use actix_web::body::{BodySize, MessageBody};
use actix_web::dev::{ServiceRequest, ServiceResponse};
use actix_web::http::Method;
use actix_web::middleware::Next;
use actix_web::{get, HttpResponse, Responder};
use prometheus::{
    exponential_buckets, register_gauge, register_gauge_vec, register_histogram,
    register_histogram_vec, register_int_counter, register_int_counter_vec, register_int_gauge,
    register_int_gauge_vec, Encoder, Gauge, GaugeVec, Histogram, HistogramVec, IntCounter,
    IntCounterVec, IntGauge, IntGaugeVec, TextEncoder,
};
use tokio::time::MissedTickBehavior;

use crate::cache::{self, TipError};
use crate::types::{BlockHeight, ChainId, Finality};
use crate::AppState;

const TARGET: &str = "metrics";

/// Label value for requests that matched no route, so 404 scanning traffic can
/// never invent new series.
const UNMATCHED: &str = "<unmatched>";

// Logical redis operation names. These are label values, so they are fixed
// `&'static str`s rather than anything derived from user input.
pub const OP_GET_LAST_BLOCK: &str = "get_last_block";
pub const OP_GET_BLOCK_AND_LAST_BLOCK: &str = "get_block_and_last_block";
pub const OP_SET_BLOCK: &str = "set_block";
pub const OP_ACQUIRE_ARCHIVE_LOCK: &str = "acquire_archive_lock";
pub const OP_WAIT_FOR_BLOCK: &str = "wait_for_block";
pub const OP_SET_MULTIPLE_BLOCKS: &str = "set_multiple_blocks";
pub const OP_TIP_OBSERVATION: &str = "tip_observation";

// ---------------------------------------------------------------- HTTP layer

static HTTP_REQUESTS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_http_requests_total",
        "HTTP requests by matched route pattern, method and response status code.",
        &["endpoint", "method", "status"]
    )
    .unwrap()
});

static HTTP_REQUEST_DURATION: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "neardata_http_request_duration_seconds",
        "End-to-end HTTP request handling duration.",
        &["endpoint", "method"],
        // The long tail is deliberate: a request for a block just past the tip
        // blocks in `wait_for_block` for up to ~11s by design.
        vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 15.0, 30.0]
    )
    .unwrap()
});

static HTTP_RESPONSE_SIZE: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "neardata_http_response_size_bytes",
        "Response body size for responses with a known length. Whole blocks are ~1 MiB.",
        &["endpoint"],
        exponential_buckets(1024.0, 4.0, 8).unwrap()
    )
    .unwrap()
});

static HTTP_REQUESTS_IN_FLIGHT: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!(
        "neardata_http_requests_in_flight",
        "HTTP requests currently being handled."
    )
    .unwrap()
});

// ---------------------------------------------------------------- Chain tip
//
// These are only ever touched by the tip poller, which does not run on archive
// nodes. A `*Vec` emits no series until a label set is touched, so on an archive
// node this whole family is simply absent from the scrape rather than reported
// as a misleading zero.

static CHAIN_TIP_LATENCY: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "neardata_chain_tip_latency_seconds",
        "Age of the latest known block at the time of the last poll, i.e. how far behind the chain this node is.",
        &["finality"]
    )
    .unwrap()
});

static CHAIN_TIP_BLOCK_HEIGHT: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!(
        "neardata_chain_tip_block_height",
        "Latest block height known to this node.",
        &["finality"]
    )
    .unwrap()
});

static CHAIN_TIP_BLOCK_TIMESTAMP: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "neardata_chain_tip_block_timestamp_seconds",
        "Header timestamp of the latest known block, as unix seconds. Alert on time() minus this rather than on the latency gauge: it keeps rising if the poller itself dies.",
        &["finality"]
    )
    .unwrap()
});

static CHAIN_TIP_UPDATED: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "neardata_chain_tip_updated_timestamp_seconds",
        "Unix time of the last successful tip poll. Staleness detector for the other chain tip gauges.",
        &["finality"]
    )
    .unwrap()
});

static CHAIN_FINALITY_LAG_BLOCKS: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!(
        "neardata_chain_finality_lag_blocks",
        "Optimistic tip height minus final tip height, when both were observed in the same poll."
    )
    .unwrap()
});

static CHAIN_BLOCKS_SEEN_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_chain_blocks_seen_total",
        "Cumulative advance of the tip height. rate() of this is blocks per second.",
        &["finality"]
    )
    .unwrap()
});

static CHAIN_TIP_REGRESSIONS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_chain_tip_regressions_total",
        "Times the tip height went backwards, e.g. after a cache flush or a reindex.",
        &["finality"]
    )
    .unwrap()
});

static CHAIN_HEAD_STALL_SECONDS: LazyLock<GaugeVec> = LazyLock::new(|| {
    register_gauge_vec!(
        "neardata_chain_head_stall_seconds",
        "Seconds since the tip height last changed.",
        &["finality"]
    )
    .unwrap()
});

static HEALTHY: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!(
        "neardata_healthy",
        "1 when the final tip is within MAX_HEALTHY_LATENCY_MS. Pinned to 1 on nodes that do not track the chain head, mirroring /health."
    )
    .unwrap()
});

static TIP_POLL_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_tip_poll_total",
        "Tip poll attempts by outcome.",
        &["finality", "result"]
    )
    .unwrap()
});

static TIP_POLL_DURATION: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "neardata_tip_poll_duration_seconds",
        "Duration of a single tip poll.",
        &["finality"],
        exponential_buckets(0.001, 2.0, 12).unwrap()
    )
    .unwrap()
});

static TIP_FULL_FETCH_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_tip_full_fetch_total",
        "Tip polls that had to fetch a whole block because the header timestamp was not in the fetched prefix.",
        &["finality"]
    )
    .unwrap()
});

// ------------------------------------------------------------------- Redis

static REDIS_COMMANDS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_redis_commands_total",
        "Redis operations by logical op and final outcome after retries.",
        &["op", "result"]
    )
    .unwrap()
});

static REDIS_COMMAND_DURATION: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "neardata_redis_command_duration_seconds",
        "Redis operation wall time including connection setup and retries. Excludes wait_for_block, whose XREAD blocks by design.",
        &["op"],
        exponential_buckets(0.0005, 2.0, 13).unwrap()
    )
    .unwrap()
});

static REDIS_FAILED_ATTEMPTS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_redis_failed_attempts_total",
        "Failed redis attempts inside the retry loop. Not the same as retries: a call that exhausts its budget records its final failure here too, having never retried it.",
        &["op"]
    )
    .unwrap()
});

static CACHE_BLOCK_WRITES_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_cache_block_writes_total",
        "Blocks written back into the redis cache after an archive read.",
        &["finality", "result"]
    )
    .unwrap()
});

// ----------------------------------------------------- Block serving/archive

static BLOCK_LOOKUP_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_block_lookup_total",
        "Block lookup attempts by outcome. Counts attempts, not requests: the lookup loop retries after waiting for a block or losing the archive read lock, so this can exceed the request rate.",
        &["finality", "outcome"]
    )
    .unwrap()
});

static BLOCK_WAIT_DURATION: LazyLock<HistogramVec> = LazyLock::new(|| {
    register_histogram_vec!(
        "neardata_block_wait_duration_seconds",
        "Time spent blocked waiting for a not-yet-produced block to arrive.",
        &["finality"],
        vec![0.05, 0.1, 0.25, 0.5, 1.0, 1.5, 2.0, 3.0, 5.0, 8.0, 12.0, 16.0]
    )
    .unwrap()
});

static ARCHIVE_READS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_archive_reads_total",
        "Local .tgz archive reads by outcome.",
        &["result"]
    )
    .unwrap()
});

static ARCHIVE_READ_DURATION: LazyLock<Histogram> = LazyLock::new(|| {
    register_histogram!(
        "neardata_archive_read_duration_seconds",
        "Time to open, gunzip and untar one local block archive.",
        exponential_buckets(0.01, 2.0, 11).unwrap()
    )
    .unwrap()
});

static ARCHIVE_BLOCKS_READ_TOTAL: LazyLock<IntCounter> = LazyLock::new(|| {
    register_int_counter!(
        "neardata_archive_blocks_read_total",
        "Block documents extracted from local archives."
    )
    .unwrap()
});

static ARCHIVE_LOCK_ATTEMPTS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_archive_lock_attempts_total",
        "Attempts to claim the shared archive read lock.",
        &["result"]
    )
    .unwrap()
});

static REDIRECTS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_redirects_total",
        "Redirect responses by reason.",
        &["kind"]
    )
    .unwrap()
});

static BLOCK_ERRORS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_block_errors_total",
        "Block error responses by error type.",
        &["type"]
    )
    .unwrap()
});

static SERVICE_ERRORS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_service_errors_total",
        "Internal service errors by kind.",
        &["kind"]
    )
    .unwrap()
});

static HEALTH_CHECKS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    register_int_counter_vec!(
        "neardata_health_checks_total",
        "Health check results.",
        &["status"]
    )
    .unwrap()
});

// -------------------------------------------------------------------- Meta

static BUILD_INFO: LazyLock<IntGaugeVec> = LazyLock::new(|| {
    register_int_gauge_vec!(
        "neardata_build_info",
        "Always 1. Carries the version and the role this instance plays in the fleet.",
        &["version", "chain", "role", "archive_index"]
    )
    .unwrap()
});

static GENESIS_BLOCK_HEIGHT: LazyLock<IntGauge> = LazyLock::new(|| {
    register_int_gauge!(
        "neardata_genesis_block_height",
        "Configured genesis block height, the lowest height this fleet serves."
    )
    .unwrap()
});

static MAX_HEALTHY_LATENCY_SECONDS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!(
        "neardata_max_healthy_latency_seconds",
        "Configured tip latency budget, so dashboards can draw the threshold from config."
    )
    .unwrap()
});

/// Registers every metric and records the static instance facts.
///
/// Forcing each `LazyLock` here means a duplicate metric name or a bad label set
/// panics at startup instead of on a worker thread during the first request that
/// happens to touch it.
pub fn init(app_state: &AppState) {
    LazyLock::force(&HTTP_REQUESTS_TOTAL);
    LazyLock::force(&HTTP_REQUEST_DURATION);
    LazyLock::force(&HTTP_RESPONSE_SIZE);
    LazyLock::force(&HTTP_REQUESTS_IN_FLIGHT);
    // CHAIN_FINALITY_LAG_BLOCKS is deliberately not forced here: it is a plain
    // gauge, so registering it would publish a flat 0 on every node that never
    // observes both tips. It registers itself on the first cycle that sees both.
    LazyLock::force(&CHAIN_TIP_LATENCY);
    LazyLock::force(&CHAIN_TIP_BLOCK_HEIGHT);
    LazyLock::force(&CHAIN_TIP_BLOCK_TIMESTAMP);
    LazyLock::force(&CHAIN_TIP_UPDATED);
    LazyLock::force(&CHAIN_BLOCKS_SEEN_TOTAL);
    LazyLock::force(&CHAIN_TIP_REGRESSIONS_TOTAL);
    LazyLock::force(&CHAIN_HEAD_STALL_SECONDS);
    LazyLock::force(&HEALTHY);
    LazyLock::force(&TIP_POLL_TOTAL);
    LazyLock::force(&TIP_POLL_DURATION);
    LazyLock::force(&TIP_FULL_FETCH_TOTAL);
    LazyLock::force(&REDIS_COMMANDS_TOTAL);
    LazyLock::force(&REDIS_COMMAND_DURATION);
    LazyLock::force(&REDIS_FAILED_ATTEMPTS_TOTAL);
    LazyLock::force(&CACHE_BLOCK_WRITES_TOTAL);
    LazyLock::force(&BLOCK_LOOKUP_TOTAL);
    LazyLock::force(&BLOCK_WAIT_DURATION);
    LazyLock::force(&ARCHIVE_READS_TOTAL);
    LazyLock::force(&ARCHIVE_READ_DURATION);
    LazyLock::force(&ARCHIVE_BLOCKS_READ_TOTAL);
    LazyLock::force(&ARCHIVE_LOCK_ATTEMPTS_TOTAL);
    LazyLock::force(&REDIRECTS_TOTAL);
    LazyLock::force(&BLOCK_ERRORS_TOTAL);
    LazyLock::force(&SERVICE_ERRORS_TOTAL);
    LazyLock::force(&HEALTH_CHECKS_TOTAL);
    LazyLock::force(&BUILD_INFO);
    LazyLock::force(&GENESIS_BLOCK_HEIGHT);
    LazyLock::force(&MAX_HEALTHY_LATENCY_SECONDS);

    let role = if app_state.is_fresh {
        "fresh"
    } else if app_state.is_latest {
        "latest"
    } else {
        "archive"
    };
    let archive_index = app_state
        .archive_config
        .as_ref()
        .map(|config| config.archive_index.to_string())
        .unwrap_or_else(|| "none".to_string());
    BUILD_INFO
        .with_label_values(&[
            env!("CARGO_PKG_VERSION"),
            &app_state.chain_id.to_string(),
            role,
            &archive_index,
        ])
        .set(1);
    GENESIS_BLOCK_HEIGHT.set(app_state.genesis_block_height as i64);
    MAX_HEALTHY_LATENCY_SECONDS.set(app_state.max_healthy_latency_ms as f64 / 1000.0);

    if !tracks_chain_head(app_state) {
        // No poller will run here, so mirror the unconditional "ok" that /health
        // returns on nodes that don't track the head.
        HEALTHY.set(1);
    }
}

/// Whether this node follows the chain head, and so has meaningful tip latency.
pub fn tracks_chain_head(app_state: &AppState) -> bool {
    app_state.is_latest || app_state.is_fresh
}

fn finality_label(finality: Finality) -> &'static str {
    match finality {
        Finality::Final => "final",
        Finality::Optimistic => "optimistic",
    }
}

fn unix_now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

// ------------------------------------------------------- Recording helpers

pub fn record_redis_op(op: &'static str, elapsed_seconds: f64, ok: bool, failed_attempts: u64) {
    // `wait_for_block` blocks on XREAD for as long as the caller asked it to, so
    // its duration says nothing about redis health and would swamp the buckets.
    if op != OP_WAIT_FOR_BLOCK {
        REDIS_COMMAND_DURATION
            .with_label_values(&[op])
            .observe(elapsed_seconds);
    }
    REDIS_COMMANDS_TOTAL
        .with_label_values(&[op, if ok { "ok" } else { "error" }])
        .inc();
    if failed_attempts > 0 {
        REDIS_FAILED_ATTEMPTS_TOTAL
            .with_label_values(&[op])
            .inc_by(failed_attempts);
    }
}

pub fn record_cache_block_writes(finality: Finality, ok: bool, blocks: u64) {
    CACHE_BLOCK_WRITES_TOTAL
        .with_label_values(&[finality_label(finality), if ok { "ok" } else { "error" }])
        .inc_by(blocks);
}

pub fn record_block_lookup(finality: Finality, outcome: &'static str) {
    BLOCK_LOOKUP_TOTAL
        .with_label_values(&[finality_label(finality), outcome])
        .inc();
}

pub fn record_block_wait(finality: Finality, elapsed: Duration) {
    BLOCK_WAIT_DURATION
        .with_label_values(&[finality_label(finality)])
        .observe(elapsed.as_secs_f64());
}

pub fn record_archive_read(found: bool, elapsed: Duration, blocks: u64) {
    ARCHIVE_READS_TOTAL
        .with_label_values(&[if found { "ok" } else { "file_not_found" }])
        .inc();
    ARCHIVE_READ_DURATION.observe(elapsed.as_secs_f64());
    if blocks > 0 {
        ARCHIVE_BLOCKS_READ_TOTAL.inc_by(blocks);
    }
}

pub fn record_archive_lock(result: &'static str) {
    ARCHIVE_LOCK_ATTEMPTS_TOTAL
        .with_label_values(&[result])
        .inc();
}

pub fn record_redirect(kind: &'static str) {
    REDIRECTS_TOTAL.with_label_values(&[kind]).inc();
}

pub fn record_block_error(error_type: &'static str) {
    BLOCK_ERRORS_TOTAL.with_label_values(&[error_type]).inc();
}

pub fn record_service_error(kind: &'static str) {
    SERVICE_ERRORS_TOTAL.with_label_values(&[kind]).inc();
}

pub fn record_health_check(status: &'static str) {
    HEALTH_CHECKS_TOTAL.with_label_values(&[status]).inc();
}

// ---------------------------------------------------------- HTTP middleware

/// Keeps the in-flight gauge correct even when a client disconnects mid-request
/// and actix drops the response future. A block request can sit in
/// `wait_for_block` for ~11s, which is exactly when clients hang up, so a manual
/// decrement after the await would leak the gauge upwards forever.
struct InFlightGuard;

impl InFlightGuard {
    fn new() -> Self {
        HTTP_REQUESTS_IN_FLIGHT.inc();
        Self
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        HTTP_REQUESTS_IN_FLIGHT.dec();
    }
}

/// The matched route pattern, not the request path: `/v0/block/123` and
/// `/v0/block/124` must land on the same series.
fn endpoint_label(request: &ServiceRequest) -> String {
    request
        .match_pattern()
        .unwrap_or_else(|| UNMATCHED.to_string())
}

/// `http::Method` accepts any RFC 7230 token, so `as_str()` would be an unbounded
/// label that anyone could inflate at will.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::HEAD => "HEAD",
        Method::POST => "POST",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::OPTIONS => "OPTIONS",
        Method::PATCH => "PATCH",
        _ => "OTHER",
    }
}

pub async fn http_metrics(
    request: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, actix_web::Error> {
    // Both have to be read before `next.call` takes ownership of the request.
    let endpoint = endpoint_label(&request);
    let method = method_label(request.method());

    let _in_flight = InFlightGuard::new();
    let started = Instant::now();
    let result = next.call(request).await;
    let elapsed = started.elapsed().as_secs_f64();

    let (status, size) = match &result {
        Ok(response) => (
            response.status(),
            match response.response().body().size() {
                BodySize::Sized(size) => Some(size),
                _ => None,
            },
        ),
        // Handlers here return `Result<impl Responder, ServiceError>`, which actix
        // turns into a status-bearing response, so this arm is not normally hit.
        // Errors raised by other middleware are, and dropping them would
        // understate the error rate.
        Err(err) => (err.as_response_error().status_code(), None),
    };

    HTTP_REQUESTS_TOTAL
        .with_label_values(&[&endpoint, method, status.as_str()])
        .inc();
    HTTP_REQUEST_DURATION
        .with_label_values(&[&endpoint, method])
        .observe(elapsed);
    if let Some(size) = size {
        HTTP_RESPONSE_SIZE
            .with_label_values(&[&endpoint])
            .observe(size as f64);
    }

    result
}

// ------------------------------------------------------------- The endpoint

#[get("/metrics")]
pub async fn get_metrics() -> impl Responder {
    let mut buffer = Vec::with_capacity(16 * 1024);
    let encoder = TextEncoder::new();
    match encoder.encode(&prometheus::gather(), &mut buffer) {
        Ok(()) => HttpResponse::Ok()
            .content_type(encoder.format_type())
            .body(buffer),
        Err(err) => {
            tracing::error!(target: TARGET, "Failed to encode metrics: {}", err);
            HttpResponse::InternalServerError().finish()
        }
    }
}

// ------------------------------------------------------------- Tip poller

pub struct TipPollerConfig {
    pub redis_client: redis::Client,
    pub chain_id: ChainId,
    /// Only fresh nodes own an optimistic tip; everyone else redirects those
    /// requests away and has no optimistic key to report.
    pub poll_optimistic: bool,
    pub max_healthy_latency_ms: u128,
    pub interval: Duration,
    pub timeout: Duration,
}

#[derive(Default)]
struct FinalityState {
    last_height: Option<BlockHeight>,
    changed_at: Option<Instant>,
}

#[derive(Default)]
struct PollState {
    final_state: FinalityState,
    optimistic_state: FinalityState,
}

impl PollState {
    fn for_finality(&mut self, finality: Finality) -> &mut FinalityState {
        match finality {
            Finality::Final => &mut self.final_state,
            Finality::Optimistic => &mut self.optimistic_state,
        }
    }
}

/// Refreshes the chain tip gauges in the background so that scraping `/metrics`
/// never touches redis.
///
/// That property is the whole point of this task: a scrape that blocks on redis
/// makes Prometheus mark the target down and drop *every* metric from it,
/// including the ones that would explain the outage.
///
/// Must be spawned before `HttpServer::new`, since that factory closure runs once
/// per worker thread and would otherwise give us one poller per worker.
pub fn spawn_tip_poller(config: TipPollerConfig) {
    // Note CHAIN_FINALITY_LAG_BLOCKS is deliberately never forced, here or in
    // init(): it registers on its first `set`, which is the first cycle that
    // actually saw both tips. Registering it any earlier would publish a
    // confident 0 - "the tips are in sync" - on every node that cannot see the
    // optimistic tip, when the truth is that it does not know.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(config.interval);
        // A slow cycle should delay the next tick, not queue up a burst of
        // catch-up ticks against a redis that is already struggling.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut connection: Option<redis::aio::MultiplexedConnection> = None;
        let mut state = PollState::default();

        loop {
            ticker.tick().await;
            let poll = poll_once(&config, &mut connection, &mut state);
            if tokio::time::timeout(config.timeout, poll).await.is_err() {
                // Without this timeout a wedged connection would freeze the
                // latency gauge at its last healthy-looking value forever.
                tracing::warn!(target: TARGET, "Tip poll timed out");
                for finality in polled_finalities(&config) {
                    TIP_POLL_TOTAL
                        .with_label_values(&[finality_label(finality), "timeout"])
                        .inc();
                }
                connection = None;
            }
        }
    });
}

fn polled_finalities(config: &TipPollerConfig) -> Vec<Finality> {
    if config.poll_optimistic {
        vec![Finality::Final, Finality::Optimistic]
    } else {
        vec![Finality::Final]
    }
}

async fn poll_once(
    config: &TipPollerConfig,
    connection: &mut Option<redis::aio::MultiplexedConnection>,
    state: &mut PollState,
) {
    let finalities = polled_finalities(config);

    if connection.is_none() {
        match config.redis_client.get_multiplexed_async_connection().await {
            Ok(established) => *connection = Some(established),
            Err(err) => {
                tracing::warn!(target: TARGET, "Failed to connect to redis: {}", err);
                for finality in &finalities {
                    TIP_POLL_TOTAL
                        .with_label_values(&[finality_label(*finality), "redis_error"])
                        .inc();
                }
                return;
            }
        }
    }
    let established = connection
        .as_mut()
        .expect("the connection was just established");

    let mut final_height = None;
    let mut optimistic_height = None;
    let mut redis_failed = false;

    for finality in &finalities {
        let finality = *finality;
        let label = finality_label(finality);
        let started = Instant::now();
        let observation = cache::tip_observation_on(established, config.chain_id, finality).await;
        TIP_POLL_DURATION
            .with_label_values(&[label])
            .observe(started.elapsed().as_secs_f64());

        let observation = match observation {
            Ok(observation) => observation,
            Err(err) => {
                if matches!(err, TipError::Redis(_)) {
                    redis_failed = true;
                }
                tracing::debug!(target: TARGET, "Tip poll for {} failed: {:?}", finality, err);
                // Deliberately leave the value gauges alone: the last known
                // reading plus a stale `..._updated_timestamp_seconds` is more
                // useful than a zero that reads as perfectly healthy.
                TIP_POLL_TOTAL
                    .with_label_values(&[label, err.result_label()])
                    .inc();
                continue;
            }
        };

        if observation.used_full_fetch {
            TIP_FULL_FETCH_TOTAL.with_label_values(&[label]).inc();
        }

        let latency_ms = cache::latency_ms_from_nanos(observation.timestamp_nanos);
        CHAIN_TIP_BLOCK_HEIGHT
            .with_label_values(&[label])
            .set(observation.height as i64);
        CHAIN_TIP_BLOCK_TIMESTAMP
            .with_label_values(&[label])
            .set(observation.timestamp_nanos as f64 / 1e9);
        CHAIN_TIP_LATENCY
            .with_label_values(&[label])
            .set(latency_ms as f64 / 1000.0);
        CHAIN_TIP_UPDATED
            .with_label_values(&[label])
            .set(unix_now_seconds());

        let finality_state = state.for_finality(finality);
        match finality_state.last_height {
            Some(previous) if observation.height > previous => {
                CHAIN_BLOCKS_SEEN_TOTAL
                    .with_label_values(&[label])
                    .inc_by(observation.height - previous);
                finality_state.changed_at = Some(Instant::now());
            }
            Some(previous) if observation.height < previous => {
                CHAIN_TIP_REGRESSIONS_TOTAL
                    .with_label_values(&[label])
                    .inc();
                finality_state.changed_at = Some(Instant::now());
            }
            Some(_) => {}
            None => finality_state.changed_at = Some(Instant::now()),
        }
        finality_state.last_height = Some(observation.height);
        if let Some(changed_at) = finality_state.changed_at {
            CHAIN_HEAD_STALL_SECONDS
                .with_label_values(&[label])
                .set(changed_at.elapsed().as_secs_f64());
        }

        match finality {
            Finality::Final => {
                HEALTHY.set(i64::from(latency_ms <= config.max_healthy_latency_ms));
                final_height = Some(observation.height);
            }
            Finality::Optimistic => optimistic_height = Some(observation.height),
        }
        TIP_POLL_TOTAL.with_label_values(&[label, "ok"]).inc();
    }

    if let (Some(final_height), Some(optimistic_height)) = (final_height, optimistic_height) {
        CHAIN_FINALITY_LAG_BLOCKS.set(optimistic_height as i64 - final_height as i64);
    }

    if redis_failed {
        *connection = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app_state() -> AppState {
        AppState {
            redis_client: redis::Client::open("redis://127.0.0.1:6379").unwrap(),
            read_config: None,
            chain_id: ChainId::Mainnet,
            genesis_block_height: 9820210,
            is_latest: false,
            is_fresh: false,
            archive_config: None,
            max_healthy_latency_ms: 15000,
        }
    }

    #[test]
    fn maps_unknown_methods_to_a_single_label() {
        assert_eq!(method_label(&Method::GET), "GET");
        assert_eq!(
            method_label(&Method::from_bytes(b"WHATEVER").unwrap()),
            "OTHER"
        );
    }

    #[test]
    fn registering_every_metric_succeeds() {
        // Registration failures (a duplicate name, a label set that doesn't match
        // the recorded values) panic inside the LazyLock initializer, so touching
        // every metric here keeps that failure out of the request path.
        init(&test_app_state());

        let families = prometheus::gather();
        assert!(families
            .iter()
            .any(|family| family.name() == "neardata_build_info"));
        // The chain tip family must stay absent on a node that doesn't track the
        // head, rather than reporting a zero latency that reads as healthy.
        // A plain gauge starts emitting 0 as soon as it is registered, so an
        // unset one reads as a confident "in sync" / "zero seconds behind".
        // These must stay unregistered until something real sets them.
        for name in [
            "neardata_chain_tip_latency_seconds",
            "neardata_chain_tip_block_height",
            "neardata_chain_finality_lag_blocks",
        ] {
            assert!(
                !families
                    .iter()
                    .any(|family| family.name() == name && !family.get_metric().is_empty()),
                "{name} must not be exposed on a node that does not track the chain head"
            );
        }
    }

    /// Pins the exact `endpoint` label values the dashboards are built on, and in
    /// doing so proves `match_pattern()` resolves from an `App::wrap` middleware,
    /// which runs before routing.
    #[actix_web::test]
    async fn labels_requests_by_route_pattern_not_by_path() {
        const BLOCK_PATTERN: &str = "/v0/block{finality:(_opt)?}/{block_height}";

        let app = actix_web::test::init_service(
            actix_web::App::new()
                .app_data(actix_web::web::Data::new(test_app_state()))
                .wrap(actix_web::middleware::from_fn(http_metrics))
                .service(actix_web::web::scope("/v0").service(crate::api::v0::get_block)),
        )
        .await;

        // The registry is global across the whole test binary, so compare deltas.
        let count = |endpoint: &str| {
            HTTP_REQUESTS_TOTAL
                .with_label_values(&[endpoint, "GET", "404"])
                .get()
        };
        let (before_pattern, before_unmatched) = (count(BLOCK_PATTERN), count(UNMATCHED));

        // Below genesis, so this is answered without ever reaching redis.
        let response = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::get()
                .uri("/v0/block/1")
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), 404);

        let response = actix_web::test::call_service(
            &app,
            actix_web::test::TestRequest::get()
                .uri("/nope")
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), 404);

        assert_eq!(
            count(BLOCK_PATTERN),
            before_pattern + 1,
            "the request should be counted under the route pattern, not under /v0/block/1"
        );
        assert_eq!(
            count(UNMATCHED),
            before_unmatched + 1,
            "an unrouted path must not be able to create a new series"
        );
    }
}
