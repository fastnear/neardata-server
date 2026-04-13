pub mod api;
pub mod cache;
#[cfg(feature = "openapi")]
pub mod openapi;
pub mod reader;
pub mod types;

use actix_web::{web, HttpResponse, Responder, Scope};

use crate::types::{BlockHeight, ChainId};

pub static INDEX_HTML: &str = include_str!("../static/index.html");
pub static SKILL_MD: &str = include_str!("../static/skill.md");

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
    /// Whether this node has the freshest blocks, but doesn't use archive files.
    pub is_fresh: bool,
    pub archive_config: Option<ArchiveConfig>,
    pub max_healthy_latency_ms: u128,
}

pub async fn serve_index() -> impl Responder {
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(INDEX_HTML)
}

pub async fn serve_skill() -> impl Responder {
    HttpResponse::Ok()
        .content_type("text/markdown; charset=utf-8")
        .body(SKILL_MD)
}

pub fn api_v0_scope() -> Scope {
    web::scope("/v0")
        .service(api::v0::get_first_block)
        .service(api::v0::get_block)
        .service(api::v0::get_last_block)
        .service(api::v0::get_block_headers)
        .service(api::v0::get_shard)
        .service(api::v0::get_chunk)
}
