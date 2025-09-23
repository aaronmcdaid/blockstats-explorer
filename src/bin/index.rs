use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;
use anyhow::Result;
use bitcoin::BlockHash;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BlockLocation {
    pub file_path: String,
    pub file_offset: u64,
    pub block_hash: BlockHash,
    pub block_size: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct BlockIndex {
    pub blocks: HashMap<u32, BlockLocation>, // height -> location
    pub tip_height: u32,
}

impl BlockIndex {
    pub fn new() -> Self {
        BlockIndex {
            blocks: HashMap::new(),
            tip_height: 0,
        }
    }

    pub fn add_block(&mut self, height: u32, location: BlockLocation) {
        self.blocks.insert(height, location);
        if height > self.tip_height {
            self.tip_height = height;
        }
    }

    pub fn get_block_location(&self, height: u32) -> Option<&BlockLocation> {
        self.blocks.get(&height)
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        bincode::serialize_into(writer, self)?;
        Ok(())
    }

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let index = bincode::deserialize_from(reader)?;
        Ok(index)
    }

    pub fn iter_reverse(&self) -> impl Iterator<Item = (&u32, &BlockLocation)> {
        let mut heights: Vec<_> = self.blocks.keys().collect();
        heights.sort_by(|a, b| b.cmp(a)); // Sort in descending order
        heights.into_iter().map(move |height| (height, &self.blocks[height]))
    }
}