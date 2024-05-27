# NEAR Data Server by FASTNEAR

## Introduction

The server provides indexed data for NEAR Protocol blockchain.

It's a simple and free alternative to the publicly
available [NEAR Lake Framework](https://github.com/near/near-lake-framework-rs) by NEAR Protocol.

FASTNEAR provides servers for both mainnet and testnet:

- Mainnet: [https://mainnet.neardata.xyz](https://mainnet.neardata.xyz)
- Testnet: [https://testnet.neardata.xyz](https://testnet.neardata.xyz)

## API

The server provides the following endpoints:

- `/v0/first_block` - Redirects to the first block after genesis.
- `/v0/block/:block_height` - Get block by the block height in a JSON format.
- `/v0/last_block/final` - Redirects to the latest finalized block.

## Usage

The server is free to use and doesn't require any authentication. The bandwidth is limited to 1 Gbps, so you may
experience throttling if there are too many parallel requests.
We use caching to reduce the load on the server and improve the response time, but it would more likely to be useful for
the latest data.

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
- The difference from NEAR Lake data is each block is served as a single JSON object, instead of the block and shards.
  Another benefit, is we include the `tx_hash` for every receipt in the `receipt_execution_outcomes`. The `tx_hash` is
  the hash of the transaction that produced the receipt.

Example:

- Genesis block (mainnet) https://mainnet.neardata.xyz/v0/block/9820210
- Regular block (mainnet) https://mainnet.neardata.xyz/v0/block/98765432
- Missing block (mainnet) https://mainnet.neardata.xyz/v0/block/115001861
- Genesis block (testnet) https://testnet.neardata.xyz/v0/block/42376888
- Regular block (testnet) https://testnet.neardata.xyz/v0/block/100000000

#### `/v0/last_block/final`

Redirects to the latest finalized block.

The block is guaranteed to exist and will be returned immediately.

Example:

- Mainnet: https://mainnet.neardata.xyz/v0/last_block/final
- Testnet: https://testnet.neardata.xyz/v0/last_block/final

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
- `READ_PATH` - The path to the directory with the block files.
- `SAVE_EVERY_N` - The number of blocks to save in the cache before saving to the disk.
- `GENESIS_BLOCK_HEIGHT` - The block height of the genesis block.

