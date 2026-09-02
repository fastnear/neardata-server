# NEAR Data Server by FASTNEAR

## Introduction

The server provides indexed data for NEAR Protocol blockchain.

It's a simple and free alternative to the publicly
available [NEAR Lake Framework](https://github.com/near/near-lake-framework-rs) by NEAR Protocol.

FASTNEAR provides servers for both mainnet and testnet:

- Mainnet: [https://mainnet.neardata.xyz](https://mainnet.neardata.xyz)
- Testnet: [https://testnet.neardata.xyz](https://testnet.neardata.xyz)

## OpenAPI Generation

The checked-in `openapi/openapi.yaml` file is generated from small typed DTOs plus a Rust-owned operation registry.

```bash
cargo run --features openapi --bin generate-openapi
cargo run --features openapi --bin generate-openapi -- --check
```

The docs pipeline is:

`neardata-server` generator -> checked-in `openapi/openapi.yaml` -> `mike-docs` split + sync -> `builder-docs` direct docs runtime

## API

The server provides the following endpoints:

- `/v0/first_block` - Redirects to the first block after genesis.
- `/v0/block/:block_height` - Get a finalized block by the block height in a JSON format.
- `/v0/block_opt/:block_height` - Get an optimistic block by the block height in a JSON format.
- `/v0/last_block/final` - Redirects to the latest finalized block.
- `/v0/last_block/optimistic` - Redirects to the latest optimistic block.

## Recommended: Rust Crate

The best option to consume Neardata is using the Rust crate: [fastnear-neardata-fetcher](https://crates.io/crates/fastnear-neardata-fetcher).
Source code is available at [github.com/fastnear/libs/tree/main/neardata-fetcher](https://github.com/fastnear/libs/tree/main/neardata-fetcher).

## Usage

The server is free to use and doesn't require any authentication. The bandwidth is limited to 1 Gbps, so you may
experience throttling if there are too many parallel requests.
We use caching to reduce the load on the server and improve the response time, but it would more likely to be useful for
the latest data.

### Rate Limits

The current rate limit is **180 requests per minute per IP**.

To increase your rate limits, get a subscription at [https://fastnear.com/](https://fastnear.com/).

### Authentication

To authenticate your requests with a FastNEAR Subscription API key, attach the following query string to the URL:

```
?apiKey={API_KEY}
```

For example:
```
https://mainnet.neardata.xyz/v0/block/98765432?apiKey=YOUR_API_KEY
```

Invalid API keys may return `401 Unauthorized` before the request reaches the neardata application itself.

> **Note:** Authentication using the `Authorization: Bearer` header requires you to manually handle redirects, since the redirect URL will not pass the header through and the redirected request will not be authenticated.

To index historical, you may read data in a sequential manner, starting from the block you need or from the genesis
block (`9820210` for mainnet) and moving forward up to the final block.

If you want to subscribe to the latest data, start from the latest finalized block and poll the server for the new
blocks incrementing the block height by one, making sure you wait for the response.

#### `/v0/first_block`

Redirects to the first block after genesis.

The block is guaranteed to exist and will be returned immediately.

Example:

- Mainnet: https://mainnet.neardata.xyz/v0/first_block
- Testnet: https://testnet.neardata.xyz/v0/first_block

#### `/v0/block/:block_height`

Returns the block by block height.

- If the block doesn't exist it returns `null`.
- If the block is not produced yet, but close to the current finalized block, the server will wait for the block to be
  produced and return it.
- Depending on deployment topology, this route may redirect to the host that owns the canonical archive range for the requested block.
- The difference from NEAR Lake data is each block is served as a single JSON object, instead of the block and shards.
  Another benefit, is we include the `tx_hash` for every receipt in the `receipt_execution_outcomes`. The `tx_hash` is
  the hash of the transaction that produced the receipt.

Example:

- Genesis block (mainnet) https://mainnet.neardata.xyz/v0/block/9820210
- Regular block (mainnet) https://mainnet.neardata.xyz/v0/block/98765432
- Missing block (mainnet) https://mainnet.neardata.xyz/v0/block/115001861
- Genesis block (testnet) https://testnet.neardata.xyz/v0/block/42376888
- Regular block (testnet) https://testnet.neardata.xyz/v0/block/100000000

#### `v0/block/:block_height/headers`

Returns a smaller part from the response including only the `block` object from the JSON document.

Example:

- Genesis block (mainnet) https://mainnet.neardata.xyz/v0/block/9820210/headers
- Regular block (mainnet) https://mainnet.neardata.xyz/v0/block/98765432/headers
- Missing block (mainnet) https://mainnet.neardata.xyz/v0/block/115001861/headers
- Genesis block (testnet) https://testnet.neardata.xyz/v0/block/42376888/headers
- Regular block (testnet) https://testnet.neardata.xyz/v0/block/100000000/headers

#### `v0/block/:block_height/chunk/:shard_id`

Returns a smaller part from the response including only the `chunk` of the requested `shard_id` part of the JSON object.

Example:

- Genesis block (mainnet) https://mainnet.neardata.xyz/v0/block/9820210/chunk/0
- Regular block (mainnet) https://mainnet.neardata.xyz/v0/block/98765432/chunk/0
- Missing block (mainnet) https://mainnet.neardata.xyz/v0/block/115001861/chunk/0
- Genesis block (testnet) https://testnet.neardata.xyz/v0/block/42376888/chunk/0
- Regular block (testnet) https://testnet.neardata.xyz/v0/block/100000000/chunk/0

#### `v0/block/:block_height/shard/:shard_id`

Returns a smaller part from the response including only the `shard` of the requested `shard_id` part of the JSON object.

Example:

- Genesis block (mainnet) https://mainnet.neardata.xyz/v0/block/9820210/shard/0
- Regular block (mainnet) https://mainnet.neardata.xyz/v0/block/98765432/shard/0
- Missing block (mainnet) https://mainnet.neardata.xyz/v0/block/115001861/shard/0
- Genesis block (testnet) https://testnet.neardata.xyz/v0/block/42376888/shard/0
- Regular block (testnet) https://testnet.neardata.xyz/v0/block/100000000/shard/0

#### `/v0/block_opt/:block_height`

Returns the optimistic block by block height.

If the deployment cannot serve the requested optimistic block directly, it may redirect to a canonical finalized or archive URL.

#### `/v0/last_block/final`

Redirects to the latest finalized block.

The block is guaranteed to exist and will be returned immediately.

Example:

- Mainnet: https://mainnet.neardata.xyz/v0/last_block/final
- Testnet: https://testnet.neardata.xyz/v0/last_block/final

#### `/v0/last_block/optimistic`

Redirects to the latest optimistic block.

The block is guaranteed to exist and will be returned immediately.

Example:

- Mainnet: https://mainnet.neardata.xyz/v0/last_block/optimistic
- Testnet: https://testnet.neardata.xyz/v0/last_block/optimistic

## Running locally

The server is built with Rust and uses the Actix Web framework.

To run the server locally, you need to have Rust installed. You can install Rust by running the following command:

```shell
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

After installing Rust, you can clone the repository and run the server:

```shell
PORT=8080 \
CHAIN_ID=mainnet \
REDIS_URL=redis://localhost:6379 \
READ_PATH=./data \
SAVE_EVERY_N=1000 \
GENESIS_BLOCK_HEIGHT=9820210 \
cargo run
```

### Environment variables

- `PORT` - The port the server will listen on.
- `CHAIN_ID` - The chain ID, either `mainnet` or `testnet`.
- `REDIS_URL` - The Redis URL for caching.
- `READ_PATH` - The path to the directory with the block files. If unset, the node
  serves no blocks from disk and redirects cache misses to the archive node that does.
- `SAVE_EVERY_N` - The number of blocks to save in the cache before saving to the disk.
- `GENESIS_BLOCK_HEIGHT` - The block height of the genesis block.
- `MAX_HEALTHY_LATENCY_MS` - How far behind the chain the latest final block may be
  before `/health` reports `unhealthy`.

Deployment topology:

- `IS_LATEST` - Whether this node carries the latest blocks and uses archive files.
  Defaults to `true` when unset. `false` means this is an archive node.
- `IS_FRESH` - Whether this node carries the freshest blocks and owns the optimistic
  tip. Defaults to `true` when unset.
- `ARCHIVE_BOUNDARIES` - Comma-separated block heights splitting the chain between
  archive nodes. Its presence enables archive routing.
- `ARCHIVE_INDEX` - Which of those ranges this node owns. `0` is genesis up to the
  first boundary; `N` is from the last boundary to the chain head.
- `DOMAIN_NAME` - The fleet domain that redirects are built against, e.g.
  `mainnet.neardata.xyz` produces `https://a2.mainnet.neardata.xyz/...`.

Metrics (see below):

- `METRICS_POLL_INTERVAL_MS` - How often the chain tip is sampled. Default `2000`.
- `METRICS_POLL_TIMEOUT_MS` - Deadline for one sampling cycle. Default `4000`.

## Metrics

`GET /metrics` serves Prometheus text exposition format. There is no separate
metrics port: the server binds to `127.0.0.1` only, so whatever reverse proxy
fronts it decides who can reach the endpoint. It exposes tip heights, request
rates and the archive topology, so restrict it there if you would rather it not
be public.

```yaml
scrape_configs:
  - job_name: neardata
    scrape_interval: 15s
    static_configs:
      - targets: ["a0.mainnet.neardata.xyz:3005", "mainnet.neardata.xyz:3005"]
```

A starter Grafana dashboard is checked in at `grafana/neardata-dashboard.json`.

### What is exposed

| Family | Notes |
| --- | --- |
| `neardata_chain_tip_*`, `neardata_chain_blocks_seen_total`, `neardata_chain_finality_lag_blocks`, `neardata_healthy` | How far behind the chain this node is. **Fresh nodes only** - see below. |
| `neardata_tip_poll_*`, `neardata_tip_full_fetch_total` | Whether the sampler that feeds those gauges is still working. |
| `neardata_http_*` | Request rate, duration, in-flight count and response size, labelled by matched route pattern. |
| `neardata_redis_*` | Redis command rate, duration and failed attempts by logical operation, for the request path only. The tip sampler runs its own connection and reports under `neardata_tip_poll_*`, so do not size Redis load from these alone. |
| `neardata_block_lookup_total`, `neardata_block_wait_duration_seconds` | How block requests were served: cache hit, archive read, or blocked waiting for a not-yet-produced block. |
| `neardata_archive_*`, `neardata_cache_block_writes_total` | Local `.tgz` read latency, hit/miss, read-lock contention and cache backfill. |
| `neardata_redirects_total`, `neardata_block_errors_total`, `neardata_service_errors_total`, `neardata_health_checks_total` | Why responses were redirects or errors. |
| `neardata_build_info`, `neardata_genesis_block_height`, `neardata_max_healthy_latency_seconds` | Static facts about this instance and its configuration. |
| `process_*` | CPU, RSS and file descriptors. Linux only. |

Labels are deliberately bounded: routes are reported as their pattern
(`/v0/block{finality:(_opt)?}/{block_height}`), never as the requested path, and
unrouted requests all collapse into a single `<unmatched>` series. Unrecognised
HTTP methods collapse into `method="OTHER"` for the same reason.

One consequence worth knowing: because the route pattern covers both spellings,
`/v0/block/N` and `/v0/block_opt/N` share a single `neardata_http_*` series, so
optimistic and final traffic cannot be separated there. Split them with
`neardata_block_lookup_total{finality}` instead, which carries the finality
label. Adding one to the HTTP histograms would double their series count for
little gain.

### Archive nodes expose no chain tip metrics

An archive node (`IS_LATEST=false` and `IS_FRESH=false`) does not follow the chain
head, so it runs no tip poller and the whole `neardata_chain_tip_*` family is
**absent** from its scrape rather than reported as zero. Likewise, only a node
with `IS_FRESH=true` owns an optimistic tip, so `finality="optimistic"` series
appear only there. Write dashboard queries with `absent()` or `or vector(0)`
rather than assuming a series exists.

### Alerting on staleness

Alert on the block timestamp, not on the latency gauge:

```promql
time() - neardata_chain_tip_block_timestamp_seconds{finality="final"} > 15
```

`neardata_chain_tip_latency_seconds` is computed when the tip is sampled, so if
the poller itself dies it freezes at whatever healthy-looking value it last had.
The expression above keeps rising in that case, because `time()` advances against
a frozen timestamp. `neardata_chain_tip_updated_timestamp_seconds` is the direct
check on poller liveness, and `neardata_tip_poll_total{result!="ok"}` says why it
is failing.

Those two separate the two ways the tip can go stale:
`neardata_chain_head_stall_seconds` rising means the chain is not moving, while
poller lag rising means this server has stopped looking. Both freeze the tip
gauges, and they need different people woken up.

The same caveat applies to `neardata_healthy`, which is set by the poller.
`/health` remains the live probe: it reads Redis on every request.
