use std::path::{Path, PathBuf};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

use clap::Parser;
use anyhow::{Result, Context, bail};
use bitcoin::{Block, consensus::Decodable};

#[derive(Parser)]
#[command(name = "simple-iterator")]
#[command(about = "Simple Bitcoin block iterator (reads blocks sequentially from blk files)")]
struct Args {
    /// Path to Bitcoin data directory
    #[arg(short, long)]
    datadir: PathBuf,

    /// Start from this block file number (default: 0)
    #[arg(long, default_value = "0")]
    start_file: u32,

    /// End at this block file number (default: 10)
    #[arg(long, default_value = "10")]
    end_file: u32,

    /// Print progress every N blocks
    #[arg(long, default_value = "1000")]
    progress_interval: u32,
}

struct BlockFileReader {
    datadir: PathBuf,
}

impl BlockFileReader {
    fn new(datadir: PathBuf) -> Self {
        BlockFileReader { datadir }
    }

    fn read_blocks_from_file(&self, file_number: u32) -> Result<Vec<Block>> {
        let file_path = self.datadir
            .join("blocks")
            .join(format!("blk{:05}.dat", file_number));

        if !file_path.exists() {
            return Ok(Vec::new()); // File doesn't exist, return empty
        }

        println!("Reading block file: {:?}", file_path);

        let mut file = File::open(&file_path)
            .context("Failed to open block file")?;

        let mut blocks = Vec::new();
        let mut position = 0u64;

        loop {
            // Seek to current position
            if let Err(_) = file.seek(SeekFrom::Start(position)) {
                break; // End of file
            }

            // Try to read magic bytes and size
            let mut header = [0u8; 8];
            match file.read_exact(&mut header) {
                Ok(_) => {},
                Err(_) => break, // End of file
            }

            let magic = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
            let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);

            // Verify magic number (mainnet: 0xD9B4BEF9)
            if magic != 0xD9B4BEF9 {
                // Try to find next valid magic by advancing one byte
                position += 1;
                continue;
            }

            // Sanity check size (blocks shouldn't be > 32MB)
            if size > 32 * 1024 * 1024 {
                position += 1;
                continue;
            }

            // Read the block data
            let mut block_data = vec![0u8; size as usize];
            match file.read_exact(&mut block_data) {
                Ok(_) => {},
                Err(_) => break, // End of file or corrupt data
            }

            // Try to parse the block
            match Block::consensus_decode(&mut std::io::Cursor::new(&block_data)) {
                Ok(block) => {
                    blocks.push(block);
                    position += 8 + size as u64; // Move to next block
                }
                Err(e) => {
                    eprintln!("Failed to decode block at position {}: {}", position, e);
                    position += 1; // Try next byte
                    continue;
                }
            }
        }

        Ok(blocks)
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Opening Bitcoin datadir: {:?}", args.datadir);

    if !args.datadir.join("blocks").exists() {
        bail!("Blocks directory not found at {:?}. Is this a valid Bitcoin datadir?",
              args.datadir.join("blocks"));
    }

    // Create block file reader
    let file_reader = BlockFileReader::new(args.datadir.clone());

    let mut total_transactions = 0u64;
    let mut total_blocks = 0u32;
    let start_time = std::time::Instant::now();

    // Iterate through block files
    for file_number in args.start_file..=args.end_file {
        println!("\nProcessing block file {}", file_number);

        match file_reader.read_blocks_from_file(file_number) {
            Ok(blocks) => {
                if blocks.is_empty() {
                    println!("Block file {} not found or empty", file_number);
                    continue;
                }

                for (block_index, block) in blocks.iter().enumerate() {
                    let tx_count = block.txdata.len();
                    total_transactions += tx_count as u64;
                    total_blocks += 1;

                    // Print first few blocks from each file
                    if block_index < 5 {
                        println!("  Block {}: Hash: {}, Transactions: {}",
                                 total_blocks, block.block_hash(), tx_count);
                    }

                    if total_blocks % args.progress_interval == 0 {
                        let elapsed = start_time.elapsed().as_secs();
                        let rate = total_blocks as f64 / elapsed.max(1) as f64;

                        println!("Progress: {} blocks processed, {:.2} blocks/sec, {} total transactions",
                                 total_blocks, rate, total_transactions);
                    }
                }

                println!("Completed file {}: {} blocks", file_number, blocks.len());
            }
            Err(e) => {
                eprintln!("Error reading block file {}: {}", file_number, e);
                continue;
            }
        }
    }

    let elapsed = start_time.elapsed();

    println!("\nSummary:");
    println!("Blocks processed: {}", total_blocks);
    println!("Total transactions: {}", total_transactions);
    println!("Time elapsed: {:.2} seconds", elapsed.as_secs_f64());
    if elapsed.as_secs_f64() > 0.0 {
        println!("Average rate: {:.2} blocks/sec", total_blocks as f64 / elapsed.as_secs_f64());
    }

    Ok(())
}