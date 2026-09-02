use dotenv::dotenv;
use std::env;
use std::time::Duration;

use actix_cors::Cors;
use actix_web::http::header;
use actix_web::{middleware, web, App, HttpServer};
use neardata_server::types::{BlockHeight, ChainId};
use neardata_server::{api, metrics};
use neardata_server::{greet, skill, AppState, ArchiveConfig, ReadConfig};
use tracing_subscriber::EnvFilter;

const DEFAULT_METRICS_POLL_INTERVAL_MS: u64 = 2000;
const DEFAULT_METRICS_POLL_TIMEOUT_MS: u64 = 4000;

fn env_duration_ms(name: &str, default_ms: u64) -> Duration {
    let millis = env::var(name)
        .ok()
        .map(|value| {
            value
                .parse()
                .unwrap_or_else(|_| panic!("Failed to parse {}", name))
        })
        .unwrap_or(default_ms);
    Duration::from_millis(millis)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    #[allow(deprecated)]
    openssl_probe::init_ssl_cert_env_vars();
    dotenv().ok();

    tracing_subscriber::fmt::Subscriber::builder()
        .with_env_filter(EnvFilter::from_default_env())
        // .with_env_filter(EnvFilter::new("debug"))
        .with_writer(std::io::stderr)
        .init();

    let chain_id = ChainId::try_from(env::var("CHAIN_ID").expect("Missing CHAIN_ID env var"))
        .expect("Failed to parse CHAIN_ID");

    let redis_client =
        redis::Client::open(env::var("REDIS_URL").expect("Missing REDIS_URL env var"))
            .expect("Failed to connect to Redis");

    let read_config = env::var("READ_PATH").ok().map(|path| ReadConfig {
        path,
        save_every_n: env::var("SAVE_EVERY_N")
            .expect("Missing SAVE_EVERY_N env var")
            .parse()
            .expect("Failed to parse SAVE_EVERY_N"),
    });

    let is_latest = env::var("IS_LATEST").map_or(true, |v| v == "true");
    let is_fresh = env::var("IS_FRESH").map_or(true, |v| v == "true");
    let archive_config = if let Ok(archive_boundaries) = env::var("ARCHIVE_BOUNDARIES") {
        let archive_boundaries: Vec<BlockHeight> = archive_boundaries
            .split(',')
            .map(|s| s.parse().expect("Failed to parse archive boundary"))
            .collect();

        let archive_index = env::var("ARCHIVE_INDEX")
            .expect("Missing ARCHIVE_INDEX env var")
            .parse()
            .expect("Failed to parse ARCHIVE_INDEX");

        Some(ArchiveConfig {
            archive_boundaries,
            domain_name: env::var("DOMAIN_NAME").expect("Missing DOMAIN_NAME env var"),
            archive_index,
        })
    } else {
        None
    };

    let genesis_block_height = env::var("GENESIS_BLOCK_HEIGHT")
        .expect("Missing GENESIS_BLOCK_HEIGHT env var")
        .parse()
        .expect("Failed to parse GENESIS_BLOCK_HEIGHT");

    let max_healthy_latency_ms = env::var("MAX_HEALTHY_LATENCY_MS")
        .expect("Missing MAX_HEALTHY_LATENCY_MS env var")
        .parse()
        .expect("Failed to parse MAX_HEALTHY_LATENCY_MS");

    let app_state = AppState {
        redis_client: redis_client.clone(),
        read_config,
        chain_id,
        genesis_block_height,
        is_latest,
        is_fresh,
        archive_config,
        max_healthy_latency_ms,
    };

    metrics::init(&app_state);
    // Spawned out here rather than in the factory closure below: that closure runs
    // once per worker thread, which would give us one poller per worker. Archive
    // nodes don't follow the chain head, so they get no poller and, by extension,
    // no chain tip metrics at all.
    if metrics::tracks_chain_head(&app_state) {
        metrics::spawn_tip_poller(metrics::TipPollerConfig {
            redis_client,
            chain_id,
            poll_optimistic: is_fresh,
            max_healthy_latency_ms,
            interval: env_duration_ms("METRICS_POLL_INTERVAL_MS", DEFAULT_METRICS_POLL_INTERVAL_MS),
            timeout: env_duration_ms("METRICS_POLL_TIMEOUT_MS", DEFAULT_METRICS_POLL_TIMEOUT_MS),
        });
    }

    HttpServer::new(move || {
        // Configure CORS middleware
        let cors = Cors::default()
            .allow_any_origin()
            .allowed_methods(vec!["GET"])
            .allowed_headers(vec![
                header::CONTENT_TYPE,
                header::AUTHORIZATION,
                header::ACCEPT,
            ])
            .max_age(3600)
            .supports_credentials();

        let api_v0 = web::scope("/v0")
            .service(api::v0::get_first_block)
            .service(api::v0::get_block)
            .service(api::v0::get_last_block)
            .service(api::v0::get_block_headers)
            .service(api::v0::get_shard)
            .service(api::v0::get_chunk);
        App::new()
            .app_data(web::Data::new(app_state.clone()))
            .wrap(cors)
            .wrap(middleware::Logger::new(
                "%{r}a \"%r\"	%s %b \"%{Referer}i\" \"%{User-Agent}i\" %T",
            ))
            .wrap(tracing_actix_web::TracingLogger::default())
            // Registered last, so it wraps everything above and measures the true
            // end-to-end time a client sees.
            .wrap(middleware::from_fn(metrics::http_metrics))
            .service(api::health)
            .service(metrics::get_metrics)
            .service(api_v0)
            .route("/", web::get().to(greet))
            .route("/skill.md", web::get().to(skill))
            .route("/SKILL.md", web::get().to(skill))
    })
    .bind(format!("127.0.0.1:{}", env::var("PORT").unwrap()))?
    .run()
    .await?;

    Ok(())
}
