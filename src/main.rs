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
    pub archive_url: String,
    pub main_url: String,
    pub start_height: BlockHeight,
    pub end_height: BlockHeight,
}

#[derive(Clone)]
pub struct AppState {
    pub redis_client: redis::Client,
    pub read_config: ReadConfig,
    pub chain_id: ChainId,
    pub genesis_block_height: BlockHeight,
    pub is_latest: bool,
    pub archive_config: Option<ArchiveConfig>,
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

    let read_config = ReadConfig {
        path: env::var("READ_PATH").expect("Missing READ_PATH env var"),
        save_every_n: env::var("SAVE_EVERY_N")
            .expect("Missing SAVE_EVERY_N env var")
            .parse()
            .expect("Failed to parse SAVE_EVERY_N"),
    };

    let is_latest = env::var("IS_LATEST").map_or(true, |v| v == "true");
    let archive_config = if let Ok(archive_url) = env::var("ARCHIVE_URL") {
        let start_height: BlockHeight = env::var("ARCHIVE_START_HEIGHT")
            .expect("Missing ARCHIVE_START_HEIGHT env var")
            .parse()
            .expect("Failed to parse ARCHIVE_START_HEIGHT");
        let end_height: BlockHeight = env::var("ARCHIVE_END_HEIGHT")
            .expect("Missing ARCHIVE_END_HEIGHT env var")
            .parse()
            .expect("Failed to parse ARCHIVE_END_HEIGHT");

        Some(ArchiveConfig {
            archive_url,
            main_url: env::var("MAIN_URL").expect("Missing MAIN_URL env var"),
            start_height,
            end_height,
        })
    } else {
        None
    };

    let genesis_block_height = env::var("GENESIS_BLOCK_HEIGHT")
        .expect("Missing GENESIS_BLOCK_HEIGHT env var")
        .parse()
        .expect("Failed to parse GENESIS_BLOCK_HEIGHT");

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
            .service(api::v0::get_opt_block)
            .service(api::v0::get_last_block);
        App::new()
            .app_data(web::Data::new(AppState {
                redis_client: redis_client.clone(),
                read_config: read_config.clone(),
                chain_id,
                genesis_block_height,
                is_latest,
                archive_config: archive_config.clone(),
            }))
            .wrap(cors)
            .wrap(middleware::Logger::new(
                "%{r}a \"%r\"	%s %b \"%{Referer}i\" \"%{User-Agent}i\" %T",
            ))
            .wrap(tracing_actix_web::TracingLogger::default())
            .service(api_v0)
            .route("/", web::get().to(greet))
    })
    .bind(format!("127.0.0.1:{}", env::var("PORT").unwrap()))?
    .run()
    .await?;

    Ok(())
}
