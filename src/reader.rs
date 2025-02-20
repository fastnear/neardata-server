use crate::types::*;
use crate::*;
use flate2::read::GzDecoder;
use std::collections::HashMap;
use std::io::Read;

const TARGET: &str = "reader";

pub fn archive_filename(
    config: &ReadConfig,
    chain_id: ChainId,
    block_height: BlockHeight,
) -> String {
    let starting_block = block_height / config.save_every_n * config.save_every_n;
    let padded_block_height = format!("{:0>12}", starting_block);
    format!(
        "{}/{}/{}/{}/{}.tgz",
        config.path,
        chain_id,
        &padded_block_height[..6],
        &padded_block_height[6..9],
        padded_block_height
    )
}

pub fn read_blocks(
    config: &ReadConfig,
    chain_id: ChainId,
    block_height: BlockHeight,
) -> Vec<(BlockHeight, Option<String>)> {
    let starting_block = block_height / config.save_every_n * config.save_every_n;
    let filename = archive_filename(config, chain_id, block_height);

    tracing::debug!(target: TARGET, "Reading blocks from {}", filename);

    let mut blocks = read_archive(&filename);
    let mut result = Vec::new();
    for i in 0..config.save_every_n {
        let block_height = starting_block + i;
        let key = format!("{:0>12}.json", block_height);
        result.push((block_height, blocks.remove(&key)));
    }
    result
}

fn read_archive(path: &str) -> HashMap<String, String> {
    if !std::path::Path::new(path).exists() {
        tracing::error!(target: TARGET, "File not found: {}", path);
        return HashMap::new();
    }
    tar::Archive::new(GzDecoder::new(std::fs::File::open(path).unwrap()))
        .entries()
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|mut e| {
            let path = e.path().unwrap().to_string_lossy().to_string();
            let mut content = String::new();
            e.read_to_string(&mut content).unwrap();
            (path, content)
        })
        .collect()
}
