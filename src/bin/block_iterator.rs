use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use clap::Parser;
use anyhow::{Result, Context, bail};
use bitcoin::{Block, BlockHash, consensus::Decodable};
use bitcoin_hashes::Hash;
use leveldb::database::Database;
use leveldb::iterator::Iterable;
use leveldb::options::{Options, ReadOptions};

#[derive(Parser)]
#[command(name = "block-iterator")]
#[command(about = "Iterate through Bitcoin blocks and count transactions")]
struct Args {
    /// Path to Bitcoin data directory
    #[arg(short, long)]
    datadir: PathBuf,

    /// Start from this block height (default: 0)
    #[arg(long, default_value = "0")]
    start_height: u32,

    /// End at this block height (default: tip)
    #[arg(long)]
    end_height: Option<u32>,

    /// Print progress every N blocks
    #[arg(long, default_value = "1000")]
    progress_interval: u32,
}

#[derive(Debug)]
struct BlockInfo {
    height: u32,
    hash: BlockHash,
    file_number: u32,
    file_offset: u32,
    block_size: u32,
}

struct BlockIndexReader {
    db: Database<Vec<u8>>,
    obfuscate_key: Vec<u8>,
}

impl BlockIndexReader {
    fn open(datadir: &Path) -> Result<Self> {
        let index_path = datadir.join("blocks").join("index");

        if !index_path.exists() {
            bail!("Block index not found at {:?}. Is this a valid Bitcoin datadir?", index_path);
        }

        let mut options = Options::new();
        options.create_if_missing = false;

        let db = Database::open(&index_path, options)
            .context("Failed to open block index database")?;

        // Read the obfuscation key
        let obfuscate_key = Self::read_obfuscate_key(&db)
            .context("Failed to read obfuscation key")?;

        Ok(BlockIndexReader { db, obfuscate_key })
    }

    fn read_obfuscate_key(db: &Database<Vec<u8>>) -> Result<Vec<u8>> {
        // The obfuscation key is stored under key [0x0e, 'o', 'b', 'f', 'u', 's', 'c', 'a', 't', 'e', '_', 'k', 'e', 'y']
        let key = b"\x0eobfuscate_key";

        match db.get(ReadOptions::new(), &key.to_vec())? {
            Some(value) => {
                if value.len() >= 8 {
                    // First 8 bytes are the actual obfuscation key
                    Ok(value[0..8].to_vec())
                } else {
                    // No obfuscation
                    Ok(vec![0u8; 8])
                }
            }
            None => {
                // No obfuscation key found, use all zeros
                Ok(vec![0u8; 8])
            }
        }
    }

    fn deobfuscate(&self, data: &[u8]) -> Vec<u8> {
        if self.obfuscate_key.iter().all(|&b| b == 0) {
            // No obfuscation
            return data.to_vec();
        }

        let mut result = Vec::with_capacity(data.len());
        for (i, &byte) in data.iter().enumerate() {
            let key_byte = self.obfuscate_key[i % self.obfuscate_key.len()];
            result.push(byte ^ key_byte);
        }
        result
    }

    fn get_tip_height(&self) -> Result<u32> {
        // Iterate through all blocks to find the highest height in the active chain
        let mut max_height = 0u32;
        let iter = self.db.iter(ReadOptions::new());

        for (key, value) in iter {
            let key = key.to_vec();
            let value = value.to_vec();
            if key.len() >= 33 && key[0] == b'b' {
                let deobfuscated = self.deobfuscate(&value);
                if let Ok(block_info) = self.parse_block_info(&deobfuscated) {
                    // Check if this block is in the active chain
                    if self.is_in_active_chain(&deobfuscated) && block_info.height > max_height {
                        max_height = block_info.height;
                    }
                }
            }
        }

        if max_height == 0 {
            bail!("Could not determine tip height");
        }

        Ok(max_height)
    }

    fn get_block_info_by_height(&self, height: u32) -> Result<Option<BlockInfo>> {
        // Iterate through all blocks to find one at the specified height in active chain
        let iter = self.db.iter(ReadOptions::new());

        for (key, value) in iter {
            let key = key.to_vec();
            let value = value.to_vec();
            if key.len() >= 33 && key[0] == b'b' {
                let deobfuscated = self.deobfuscate(&value);
                if let Ok(mut block_info) = self.parse_block_info(&deobfuscated) {
                    if block_info.height == height && self.is_in_active_chain(&deobfuscated) {
                        let hash_bytes: [u8; 32] = key[1..33].try_into()
                            .context("Invalid hash length")?;
                        let hash = BlockHash::from_byte_array(hash_bytes);

                        block_info.hash = hash;
                        return Ok(Some(block_info));
                    }
                }
            }
        }

        Ok(None)
    }

    fn parse_block_info(&self, value: &[u8]) -> Result<BlockInfo> {
        if value.len() < 32 {
            bail!("Block info too short: {} bytes", value.len());
        }

        // Bitcoin Core's block index format (simplified):
        // - 4 bytes: height
        // - 4 bytes: status
        // - 4 bytes: tx count
        // - 4 bytes: file number (where block is stored)
        // - 4 bytes: data pos (offset in file)
        // - 4 bytes: undo pos
        // - 80 bytes: block header
        // + more fields...

        let height = u32::from_le_bytes([value[0], value[1], value[2], value[3]]);

        // Skip status (4 bytes) and tx count (4 bytes)
        let file_number = u32::from_le_bytes([value[12], value[13], value[14], value[15]]);
        let file_offset = u32::from_le_bytes([value[16], value[17], value[18], value[19]]);

        // For block size, we'll need to calculate it differently or read from block header
        // For now, we'll set it to 0 and calculate when reading the actual block file
        let block_size = 0; // Will be read from block file header

        Ok(BlockInfo {
            height,
            hash: BlockHash::from_byte_array([0u8; 32]), // Will be filled by caller
            file_number,
            file_offset,
            block_size,
        })
    }

    fn is_in_active_chain(&self, value: &[u8]) -> bool {
        if value.len() < 8 {
            return false;
        }

        // Read status flags (4 bytes at offset 4)
        let status = u32::from_le_bytes([value[4], value[5], value[6], value[7]]);

        // BLOCK_VALID_CHAIN = 0x04
        // BLOCK_VALID_SCRIPTS = 0x10
        // We want blocks that are in the active chain
        (status & 0x04) != 0
    }
}

struct BlockFileReader {
    datadir: PathBuf,
}

impl BlockFileReader {
    fn new(datadir: PathBuf) -> Self {
        BlockFileReader { datadir }
    }

    fn read_block(&self, block_info: &BlockInfo) -> Result<Block> {
        let file_path = self.datadir
            .join("blocks")
            .join(format!("blk{:05}.dat", block_info.file_number));

        if !file_path.exists() {
            bail!("Block file not found: {:?}", file_path);
        }

        let mut file = File::open(&file_path)
            .context("Failed to open block file")?;

        // Seek to the block position
        file.seek(SeekFrom::Start(block_info.file_offset as u64))
            .context("Failed to seek to block position")?;

        // Read magic bytes (4 bytes) and block size (4 bytes)
        let mut header = [0u8; 8];
        file.read_exact(&mut header)
            .context("Failed to read block header")?;

        let magic = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);

        // Verify magic number (mainnet: 0xD9B4BEF9)
        if magic != 0xD9B4BEF9 {
            bail!("Invalid magic number: 0x{:08x}", magic);
        }

        // Read the block data
        let mut block_data = vec![0u8; size as usize];
        file.read_exact(&mut block_data)
            .context("Failed to read block data")?;

        // Parse the block
        let mut cursor = std::io::Cursor::new(block_data);
        let block = Block::consensus_decode(&mut cursor)
            .context("Failed to decode block")?;

        Ok(block)
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Opening Bitcoin datadir: {:?}", args.datadir);

    // Open block index (handles XOR obfuscation)
    let index_reader = BlockIndexReader::open(&args.datadir)
        .context("Failed to open block index")?;

    // Get tip height
    let tip_height = index_reader.get_tip_height()
        .context("Failed to get tip height")?;

    let end_height = args.end_height.unwrap_or(tip_height);

    if args.start_height > end_height {
        bail!("Start height {} is greater than end height {}", args.start_height, end_height);
    }

    println!("Blockchain tip at height: {}", tip_height);
    println!("Iterating from height {} to {}", args.start_height, end_height);

    // Create block file reader
    let file_reader = BlockFileReader::new(args.datadir.clone());

    let mut total_transactions = 0u64;
    let start_time = std::time::Instant::now();

    // Iterate through blocks
    for height in args.start_height..=end_height {
        if let Some(block_info) = index_reader.get_block_info_by_height(height)? {
            match file_reader.read_block(&block_info) {
                Ok(block) => {
                    let tx_count = block.txdata.len();
                    total_transactions += tx_count as u64;

                    println!("Height: {}, Hash: {}, Transactions: {}",
                             height, block_info.hash, tx_count);

                    if height % args.progress_interval == 0 && height > args.start_height {
                        let elapsed = start_time.elapsed().as_secs();
                        let blocks_processed = height - args.start_height + 1;
                        let rate = blocks_processed as f64 / elapsed.max(1) as f64;

                        println!("Progress: {} blocks processed, {:.2} blocks/sec, {} total transactions",
                                 blocks_processed, rate, total_transactions);
                    }
                }
                Err(e) => {
                    eprintln!("Error reading block at height {}: {}", height, e);
                    continue;
                }
            }
        } else {
            eprintln!("Block not found at height {}", height);
        }
    }

    let elapsed = start_time.elapsed();
    let blocks_processed = end_height - args.start_height + 1;

    println!("\nSummary:");
    println!("Blocks processed: {}", blocks_processed);
    println!("Total transactions: {}", total_transactions);
    println!("Time elapsed: {:.2} seconds", elapsed.as_secs_f64());
    if elapsed.as_secs_f64() > 0.0 {
        println!("Average rate: {:.2} blocks/sec", blocks_processed as f64 / elapsed.as_secs_f64());
    }

    Ok(())
}