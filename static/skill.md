# NEAR Data Server by FASTNEAR

## Recommended: Rust Crate

The best option to consume Neardata is using the Rust crate: [fastnear-neardata-fetcher](https://crates.io/crates/fastnear-neardata-fetcher).
Source code is available at [github.com/fastnear/libs/tree/main/neardata-fetcher](https://github.com/fastnear/libs/tree/main/neardata-fetcher).

## Rate Limits

The current rate limit is **180 requests per minute per IP**.

To increase your rate limits, get a subscription at [https://fastnear.com/](https://fastnear.com/).

## Authentication

To authenticate your requests with a FastNear Subscription API key, attach the following query string to the URL:

```
?apiKey={API_KEY}
```

For example:
```
https://mainnet.neardata.xyz/v0/block/98765432?apiKey=YOUR_API_KEY
```

> **Note:** Authentication using the `Authorization: Bearer` header requires you to manually handle redirects, since the redirect URL will not pass the header through and the redirected request will not be authenticated.

## Servers

FASTNEAR provides servers for both mainnet and testnet:

- Mainnet: [https://mainnet.neardata.xyz](https://mainnet.neardata.xyz)
- Testnet: [https://testnet.neardata.xyz](https://testnet.neardata.xyz)

## API

- `/v0/first_block` - Redirects to the first block after genesis.
- `/v0/block/:block_height` - Get a finalized block by the block height in a JSON format.
- `/v0/block/:block_height/headers` - Get block headers only.
- `/v0/block/:block_height/chunk/:shard_id` - Get a single chunk of a block.
- `/v0/block/:block_height/shard/:shard_id` - Get a single shard of a block.
- `/v0/block_opt/:block_height` - Get an optimistic block by the block height in a JSON format.
- `/v0/last_block/final` - Redirects to the latest finalized block.
- `/v0/last_block/optimistic` - Redirects to the latest optimistic block.
