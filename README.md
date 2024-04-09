# NEAR Data Server by FASTNEAR

## Introduction

The server provides indexed data for NEAR Protocol blockchain.

It's a simple and free alternative to the publicly
available [NEAR Lake Framework](https://github.com/near/near-lake-framework-rs) by NEAR Protocol.

FASTNEAR provides servers for both mainnet and testnet:

- Mainnet: [https://mainnet.neardata.xyz](https://mainnet.neardata.xyz)
- Testnet (STILL CATCHING UP, NOT READY FOR PROD): [https://testnet.neardata.xyz](https://testnet.neardata.xyz)

## API

The server provides the following endpoints:

- `/v0/block/:block_height` - Get block by the block height in a JSON format.
- `/v0/last_block/final` - Redirects to the latest finalized block.

## Usage

The server is free to use and doesn't require any authentication. The bandwidth is limited to 1 Gbps, so you may
experience throttling if there are too many parallel requests.
We use caching to reduce the load on the server and improve the response time, but it would more likely to be useful for
the latest data.

To index historical, you may read data in a sequential manner, starting from the block you need or from the genesis
block (`9820226` for mainnet) and moving forward up to the final block.

If you want to subscribe to the latest data, start from the latest finalized block and poll the server for the new
blocks incrementing the block height by one, making sure you wait for the response.

#### `/v0/block/:block_height`

Returns the block by block height.

- If the block doesn't exist it return `null`.
- If the block is not produced yet, but close to the current finalized block, the server will wait for the block to be
  produced and return it.

Example:

- Regular block (mainnet) https://mainnet.neardata.xyz/v0/block/98765432
- Missing block (mainnet) https://mainnet.neardata.xyz/v0/block/115001861
- Regular block (testnet) https://testnet.neardata.xyz/v0/block/100000000

#### `/v0/last_block/final`

Redirects to the latest finalized block.

The block is guaranteed to exist and will be returned immediately.

Example:

- Mainnet: https://mainnet.neardata.xyz/v0/last_block/final
- Testnet: https://testnet.neardata.xyz/v0/last_block/final


