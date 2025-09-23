use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::collections::HashMap;
use std::io;
use bitcoin::BlockHash;
use bitcoin::hashes::Hash;
use bitcoin::{Amount, Transaction};
use block_parser::BlockFileReader;
use index::{BlockIndex, BlockLocation};

const INDEX_PATH: &str = "blockchain.idx";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum BlockHeight {
    NotYetKnown,
    Known(u32),
    Orphaned,
}

mod block_parser;
mod index;

#[derive(Parser)]
#[command(name = "blooming-fast-utxo-set")]
#[command(about = "Fast Bitcoin blockchain indexer and analyzer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    BuildIndex {
        #[arg(long, help = "Path to Bitcoin data directory containing blk*.dat files")]
        datadir: PathBuf,
    },
    Iterate {
        #[arg(long, help = "Path to Bitcoin data directory containing blk*.dat files")]
        datadir: PathBuf,
        #[arg(long, help = "Starting block height (default: tip)")]
        start_height: Option<u32>,
        #[arg(long, help = "Ending block height (default: 0)")]
        end_height: Option<u32>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::BuildIndex { datadir } => {
            println!("Building index from data directory: {}", datadir.display());
            build_index(datadir)?;
        }
        Commands::Iterate { datadir, start_height, end_height } => {
            println!("Iterating blocks from {:?} to {:?}", start_height, end_height);
            iterate_blocks(datadir, start_height, end_height)?;
        }
    }

    Ok(())
}

fn load_xor_key(datadir: &PathBuf) -> anyhow::Result<[u8; 8]> {
    let xor_path = datadir.join("xor.dat");
    if xor_path.exists() {
        let mut xor_key = [0u8; 8];
        let mut file = std::fs::File::open(xor_path)?;
        std::io::Read::read_exact(&mut file, &mut xor_key)?;
        println!("Loaded XOR key: {:02x?}", xor_key);
        Ok(xor_key)
    } else {
        println!("No xor.dat found, assuming no obfuscation");
        Ok([0u8; 8])
    }
}

fn get_block_reward(height: u32) -> Amount {
    // Bitcoin block reward halves every 210,000 blocks
    let halvings = height / 210_000;

    // Initial reward was 50 BTC
    let initial_reward_sats = 50 * 100_000_000u64; // 50 BTC in satoshis

    // After 33 halvings, reward becomes 0
    if halvings >= 33 {
        return Amount::ZERO;
    }

    // Calculate reward after halvings: reward = initial_reward / (2^halvings)
    let reward_sats = initial_reward_sats >> halvings;
    Amount::from_sat(reward_sats)
}

fn calculate_block_fees(transactions: &[Transaction], height: u32) -> Amount {
    if transactions.is_empty() {
        return Amount::ZERO;
    }

    let coinbase = &transactions[0];

    // Sum all outputs in the coinbase transaction
    let coinbase_output_value: u64 = coinbase.output.iter()
        .map(|output| output.value.to_sat())
        .sum();

    // Get the exact block reward for this height
    let block_reward = get_block_reward(height);

    // Fees = coinbase_outputs - block_reward
    if coinbase_output_value >= block_reward.to_sat() {
        Amount::from_sat(coinbase_output_value - block_reward.to_sat())
    } else {
        Amount::ZERO
    }
}

fn build_index(datadir: PathBuf) -> anyhow::Result<()> {
    println!("Building index from data directory: {}", datadir.display());

    // Check if index already exists
    if std::path::Path::new(INDEX_PATH).exists() {
        println!("Warning: Index file '{}' already exists.", INDEX_PATH);
        println!("This will overwrite the existing index. Continue? (y/N)");

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().to_lowercase().starts_with('y') {
            println!("Index build cancelled.");
            return Ok(());
        }

        println!("Overwriting existing index...");
    }

    // Load XOR key for deobfuscation
    let xor_key = load_xor_key(&datadir)?;

    // Find all blk*.dat files
    let mut blk_files = Vec::new();
    for entry in std::fs::read_dir(&datadir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let file_name_str = file_name.to_string_lossy();

        if file_name_str.starts_with("blk") && file_name_str.ends_with(".dat") {
            blk_files.push(entry.path());
        }
    }

    blk_files.sort();
    println!("Found {} block files", blk_files.len());

    // First pass: collect all blocks and their prev_hash relationships
    let mut blocks_by_hash: HashMap<BlockHash, (u64, String, BlockHash, BlockHeight)> = HashMap::new(); // hash -> (offset, file_path, prev_hash, height)
    let mut genesis_hash: Option<BlockHash> = None;

    for blk_file in &blk_files {
        println!("Processing file: {}", blk_file.display());
        let mut reader = BlockFileReader::new_with_xor_key(blk_file, xor_key)?;
        let mut block_count = 0;

        while let Some((header, offset)) = reader.read_next_header()? {
            let block_hash = header.block_hash();
            let prev_hash = header.prev_blockhash;

            blocks_by_hash.insert(
                block_hash,
                (offset, blk_file.to_string_lossy().to_string(), prev_hash, BlockHeight::NotYetKnown)
            );

            // Check if this is the genesis block (prev_hash is all zeros)
            if prev_hash == BlockHash::all_zeros() {
                genesis_hash = Some(block_hash);
                println!("Found genesis block: {}", block_hash);
            }

            block_count += 1;
        }

        println!("  Found {} blocks", block_count);
    }

    println!("Total blocks collected: {}", blocks_by_hash.len());

    // Verify genesis block was found and set its height to 0
    let _genesis = match genesis_hash {
        Some(hash) => {
            if let Some((_, _, _, height)) = blocks_by_hash.get_mut(&hash) {
                *height = BlockHeight::Known(0);
                println!("Set genesis block height to 0: {}", hash);
            }
            hash
        },
        None => return Err(anyhow::anyhow!("Genesis block not found - no block with prev_hash = all zeros")),
    };

    // Iterative function to calculate and cache block heights. It's called
    // once for each block for which we have a hash
    fn get_block_height(
        start_hash: &BlockHash,
        blocks_map: &mut HashMap<BlockHash, (u64, String, BlockHash, BlockHeight)>
    ) -> anyhow::Result<BlockHeight> {
        // Check if height is already calculated
        if let Some((_, _, _, height)) = blocks_map.get(start_hash) {
            match height {
                BlockHeight::Known(_) | BlockHeight::Orphaned => return Ok(*height),
                BlockHeight::NotYetKnown => {} // Continue to calculate
            }
        }

        // Build the chain path using an explicit stack
        let mut path = Vec::new();
        let mut current_hash = *start_hash;

        // Traverse backwards to find a block with known height
        loop {
            if let Some((_, _, _, height)) = blocks_map.get(&current_hash) {
                match height {
                    BlockHeight::Known(_) => {
                        // Found a block with known height, start calculating from here
                        break;
                    }
                    BlockHeight::Orphaned => {
                        // Parent is orphaned, so this chain is also orphaned
                        for &hash in &path {
                            blocks_map.get_mut(&hash).unwrap().3 = BlockHeight::Orphaned;
                        }
                        return Ok(BlockHeight::Orphaned);
                    }
                    BlockHeight::NotYetKnown => {} // Continue traversing
                }
            } else {
                // Block not found - this chain is orphaned
                for &hash in &path {
                    blocks_map.get_mut(&hash).unwrap().3 = BlockHeight::Orphaned;
                }
                blocks_map.get_mut(start_hash).unwrap().3 = BlockHeight::Orphaned;
                return Ok(BlockHeight::Orphaned);
            }

            // Get the block info
            let (_, _, prev_hash, _) = blocks_map.get(&current_hash)
                .ok_or_else(|| anyhow::anyhow!("Block hash not found: {}", current_hash))?
                .clone();

            path.push(current_hash);
            current_hash = prev_hash;
        }

        // Now calculate heights going forward from the known parent
        let current_height = match blocks_map.get(&current_hash).unwrap().3 {
            BlockHeight::Known(h) => h,
            _ => return Err(anyhow::anyhow!("Unexpected state")),
        };

        // Process the path in reverse order (from parent to child)
        let mut height = current_height;
        for &hash in path.iter().rev() {
            height += 1;
            blocks_map.get_mut(&hash).unwrap().3 = BlockHeight::Known(height);
        }

        Ok(BlockHeight::Known(height))
    }

    // Calculate heights for all blocks
    println!("Calculating block heights...");
    let all_hashes: Vec<BlockHash> = blocks_by_hash.keys().cloned().collect();
    for hash in all_hashes {
        get_block_height(&hash, &mut blocks_by_hash)?;
    }

    // Find the tip block (highest height)
    let tip_height = blocks_by_hash.values()
        .filter_map(|(_, _, _, height)| match height {
            BlockHeight::Known(h) => Some(*h),
            _ => None,
        })
        .max();

    let tip_hash = if let Some(max_height) = tip_height {
        // Find all blocks at tip height
        let tip_blocks: Vec<BlockHash> = blocks_by_hash.iter()
            .filter_map(|(hash, (_, _, _, height))| {
                match height {
                    BlockHeight::Known(h) if *h == max_height => Some(*hash),
                    _ => None,
                }
            })
            .collect();

        println!("Tip height: {}", max_height);
        println!("Blocks at tip height: {}", tip_blocks.len());

        if tip_blocks.len() == 1 {
            let tip = tip_blocks[0];
            println!("Tip block: {}", tip);
            tip
        } else {
            println!("Error: Multiple blocks at tip height (blockchain fork):");
            for block in &tip_blocks {
                println!("  {}", block);
            }
            return Err(anyhow::anyhow!("Cannot determine unique tip block - found {} blocks at height {}", tip_blocks.len(), max_height));
        }
    } else {
        return Err(anyhow::anyhow!("No blocks with known heights found"));
    };

    // Build the blockchain index starting from tip and working backwards
    println!("Building blockchain index from tip...");
    let mut block_index = BlockIndex::new();

    let mut current_hash = tip_hash;
    let mut current_height = match blocks_by_hash.get(&current_hash).unwrap().3 {
        BlockHeight::Known(h) => h,
        _ => return Err(anyhow::anyhow!("Tip block doesn't have known height")),
    };

    // Build index by following the chain backwards from tip to genesis
    loop {
        if let Some((offset, file_path, prev_hash, _)) = blocks_by_hash.get(&current_hash) {
            let location = BlockLocation {
                file_path: file_path.clone(),
                file_offset: *offset,
                block_hash: current_hash,
            };

            block_index.add_block(current_height, location);

            // Move to previous block
            if *prev_hash == BlockHash::all_zeros() {
                // Reached genesis, we're done
                break;
            }

            current_hash = *prev_hash;
            current_height -= 1;

            if current_height % 10000 == 0 {
                println!("Added block at height {}", current_height);
            }
        } else {
            return Err(anyhow::anyhow!("Chain broken at height {}, missing block: {}", current_height, current_hash));
        }
    }

    println!("Built index for {} blocks", block_index.blocks.len());

    // Save index to file
    block_index.save_to_file(INDEX_PATH)?;
    println!("Index saved to: {}", INDEX_PATH);

    Ok(())
}

fn iterate_blocks(datadir: PathBuf, start_height: Option<u32>, end_height: Option<u32>) -> anyhow::Result<()> {
    // Load the index
    let block_index = BlockIndex::load_from_file(INDEX_PATH)?;

    // Load XOR key for deobfuscation
    let xor_key = load_xor_key(&datadir)?;

    let start = start_height.unwrap_or(block_index.tip_height);
    let end = end_height.unwrap_or(0);

    println!("Iterating blocks from height {} to {} (reverse order)", start, end);
    println!("Index contains {} blocks, tip height: {}", block_index.blocks.len(), block_index.tip_height);

    let mut processed_count = 0;

    for (height, location) in block_index.iter_reverse() {
        // Skip blocks outside our range
        if *height > start || *height < end {
            continue;
        }

        // Read the block from file
        let mut reader = BlockFileReader::new_with_xor_key(&location.file_path, xor_key)?;
        reader.seek_to_offset(location.file_offset)?;

        if let Some((block, _offset)) = reader.read_next_block()? {
            let tx_count = block.txdata.len();
            let fees = calculate_block_fees(&block.txdata, *height);
            let fees_btc = fees.to_btc();

            println!("Height: {}, Transactions: {}, Fees: {:.8} BTC", height, tx_count, fees_btc);

            processed_count += 1;

            // Optional: limit output for very large ranges
            if processed_count % 1000 == 0 {
                println!("  ... processed {} blocks", processed_count);
            }
        } else {
            eprintln!("Warning: Could not read block at height {}", height);
        }
    }

    println!("Completed iteration. Processed {} blocks.", processed_count);
    Ok(())
}

