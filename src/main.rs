mod api;
mod cache;
mod reader;
mod types;

use dotenv::dotenv;
use std::env;

use crate::types::{BlockHeight, ChainId};
use actix_cors::Cors;
use actix_web::http::header;
use actix_web::{get, middleware, web, App, HttpRequest, HttpResponse, HttpServer, Responder};
use tracing_subscriber::EnvFilter;

pub static INDEX_HTML: &str = include_str!("../static/index.html");

#[derive(Clone)]
pub struct ReadConfig {
    pub path: String,
    pub save_every_n: u64,
}

#[derive(Clone)]
pub struct ArchiveConfig {
    pub archive_boundaries: Vec<BlockHeight>,
    pub domain_name: String,
    /// The index of the archive boundary that this node is responsible for.
    /// E.g. If there are 2 boundaries:
    /// - `0` -> means from genesis to the first archive boundary (exclusive).
    /// - `1` -> means from the first archive boundary to the second archive boundary (exclusive).
    /// - `2` -> means from the second archive boundary to the blockchain head.
    pub archive_index: usize,
}

#[derive(Clone)]
pub struct AppState {
    pub redis_client: redis::Client,
    pub read_config: Option<ReadConfig>,
    pub chain_id: ChainId,
    pub genesis_block_height: BlockHeight,
    /// Whether this node has the latest blocks and uses archive files.
    /// If not, it means this is an archive node.
    pub is_latest: bool,
    /// Whether this node has the freshest blocks, but doesn't use archive files
    pub is_fresh: bool,
    pub archive_config: Option<ArchiveConfig>,
    pub max_healthy_latency_ms: u128,
}

async fn greet() -> impl Responder {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(INDEX_HTML)
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
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
            .app_data(web::Data::new(AppState {
                redis_client: redis_client.clone(),
                read_config: read_config.clone(),
                chain_id,
                genesis_block_height,
                is_latest,
                is_fresh,
                archive_config: archive_config.clone(),
                max_healthy_latency_ms,
            }))
            .wrap(cors)
            .wrap(middleware::Logger::new(
                "%{r}a \"%r\"	%s %b \"%{Referer}i\" \"%{User-Agent}i\" %T",
            ))
            .wrap(tracing_actix_web::TracingLogger::default())
            .service(api::health)
            .service(api_v0)
            .route("/", web::get().to(greet))
    })
    .bind(format!("127.0.0.1:{}", env::var("PORT").unwrap()))?
    .run()
    .await?;

    Ok(())
}
