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
        #[arg(long, default_value = "~/.bitcoin", help = "Path to Bitcoin data directory (parent of blocks/ folder)")]
        datadir: PathBuf,
    },
    Iterate {
        #[arg(long, default_value = "~/.bitcoin", help = "Path to Bitcoin data directory (parent of blocks/ folder)")]
        datadir: PathBuf,
        #[arg(long, help = "Starting block height (default: tip)")]
        start_height: Option<u32>,
        #[arg(long, help = "Ending block height (default: 0)")]
        end_height: Option<u32>,
    },
    Export {
        #[arg(long, default_value = "~/.bitcoin", help = "Path to Bitcoin data directory (parent of blocks/ folder)")]
        datadir: PathBuf,
        #[arg(help = "Output Arrow file path")]
        filename: PathBuf,
        #[arg(help = "Column names to export (e.g., height tx_count fee_avg)")]
        columns: Vec<String>,
        #[arg(long, help = "Maximum block height to export (default: tip)")]
        max_height: Option<u32>,
    },
}

fn expand_tilde(path: &PathBuf) -> PathBuf {
    if path.to_string_lossy().starts_with("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            let mut expanded = PathBuf::from(home);
            expanded.push(path.strip_prefix("~/").unwrap_or(path));
            return expanded;
        }
    }
    path.clone()
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::BuildIndex { datadir } => {
            let expanded_datadir = expand_tilde(&datadir);
            println!("Building index from data directory: {}", expanded_datadir.display());
            build_index(expanded_datadir)?;
        }
        Commands::Iterate { datadir, start_height, end_height } => {
            let expanded_datadir = expand_tilde(&datadir);
            println!("Iterating blocks from {:?} to {:?}", start_height, end_height);
            iterate_blocks(expanded_datadir, start_height, end_height)?;
        }
        Commands::Export { datadir, filename, columns, max_height } => {
            let expanded_datadir = expand_tilde(&datadir);
            println!("Exporting columns {:?} to {}", columns, filename.display());
            export_arrow_file(expanded_datadir, filename, columns, max_height)?;
        }
    }

    Ok(())
}

fn load_xor_key(datadir: &PathBuf) -> anyhow::Result<[u8; 8]> {
    let xor_path = datadir.join("blocks").join("xor.dat");
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

    // Find all blk*.dat files in the blocks subdirectory
    let blocks_dir = datadir.join("blocks");
    let mut blk_files = Vec::new();
    for entry in std::fs::read_dir(&blocks_dir)? {
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

    // Calculate and cache block heights using a two-phase approach:
    // 1. Build stack of hashes backwards until we find a known height or orphan
    // 2. Unwind stack forwards, setting heights incrementally
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

        // Phase 1: Build stack backwards until we find a block with known status
        let mut stack = Vec::new();
        let mut current_hash = *start_hash;

        loop {
            // Add current hash to stack
            stack.push(current_hash);

            // Check if current block exists and get its info
            let (_, _, prev_hash, height) = match blocks_map.get(&current_hash) {
                Some(info) => info.clone(),
                None => {
                    // Block not found - entire chain is orphaned
                    for &hash in &stack {
                        if let Some(entry) = blocks_map.get_mut(&hash) {
                            entry.3 = BlockHeight::Orphaned;
                        }
                    }
                    return Ok(BlockHeight::Orphaned);
                }
            };

            // Check if we've reached a block with known status
            match height {
                BlockHeight::Known(_) => {
                    // Found a block with known height - we can start calculating from here
                    break;
                }
                BlockHeight::Orphaned => {
                    // Found an orphaned block - entire chain is orphaned
                    for &hash in &stack {
                        blocks_map.get_mut(&hash).unwrap().3 = BlockHeight::Orphaned;
                    }
                    return Ok(BlockHeight::Orphaned);
                }
                BlockHeight::NotYetKnown => {
                    // Continue traversing backwards
                    current_hash = prev_hash;
                }
            }
        }

        // Phase 2: Unwind stack, setting heights incrementally
        let base_height = match blocks_map.get(&current_hash).unwrap().3 {
            BlockHeight::Known(h) => h,
            _ => return Err(anyhow::anyhow!("Expected known height but found something else")),
        };

        // Remove the base block from stack since it already has a known height
        stack.pop();

        // Process stack in reverse order (from oldest to newest block)
        let mut height = base_height;
        for &hash in stack.iter().rev() {
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

// Column extraction functions
type ColumnExtractor = fn(&bitcoin::Block, u32) -> f64;

fn get_column_extractor(column_name: &str) -> anyhow::Result<ColumnExtractor> {
    match column_name {
        "height" => Ok(|_block, height| height as f64),
        "timestamp" => Ok(|block, _height| block.header.time as f64),
        "tx_count" => Ok(|block, _height| block.txdata.len() as f64),
        "fee_avg" => Ok(|block, height| {
            let fees = calculate_block_fees(&block.txdata, height);
            let tx_count = block.txdata.len().saturating_sub(1); // Exclude coinbase
            if tx_count > 0 {
                fees.to_sat() as f64 / tx_count as f64
            } else {
                0.0
            }
        }),
        "block_size" => Ok(|block, _height| {
            bitcoin::consensus::serialize(block).len() as f64
        }),
        _ => Err(anyhow::anyhow!("Unknown column: {}", column_name)),
    }
}

fn export_arrow_file(
    datadir: PathBuf,
    filename: PathBuf,
    columns: Vec<String>,
    max_height: Option<u32>,
) -> anyhow::Result<()> {
    use arrow::array::{Float64Builder, RecordBatch, RecordBatchWriter};
    use arrow::datatypes::{DataType, Field, Schema};
    use arrow_ipc::writer::FileWriter;
    use std::fs::File;
    use std::sync::Arc;

    // Load the index
    let block_index = BlockIndex::load_from_file(INDEX_PATH)?;
    let xor_key = load_xor_key(&datadir)?;

    // Determine height range
    let tip_height = block_index.tip_height;
    let export_max_height = max_height.unwrap_or(tip_height);
    let export_min_height = 0u32;

    println!("Exporting {} columns from height {} to {}",
             columns.len(), export_min_height, export_max_height);

    // Validate columns and get extractors
    let mut extractors = Vec::new();
    for column in &columns {
        extractors.push(get_column_extractor(column)?);
    }

    // Create Arrow schema
    let fields: Vec<Field> = columns.iter()
        .map(|name| Field::new(name, DataType::Float64, false))
        .collect();
    let schema = Arc::new(Schema::new(fields));

    // Create builders for each column
    let mut builders: Vec<Float64Builder> = columns.iter()
        .map(|_| Float64Builder::new())
        .collect();

    // Process blocks and collect data
    let mut processed_count = 0;
    for height in export_min_height..=export_max_height {
        if let Some(location) = block_index.blocks.get(&height) {
            let mut reader = BlockFileReader::new_with_xor_key(&location.file_path, xor_key)?;
            reader.seek_to_offset(location.file_offset)?;

            if let Some((block, _offset)) = reader.read_next_block()? {
                // Extract values for each column
                for (i, extractor) in extractors.iter().enumerate() {
                    let value = extractor(&block, height);
                    builders[i].append_value(value);
                }

                processed_count += 1;
                if processed_count % 10000 == 0 {
                    println!("Processed {} blocks...", processed_count);
                }
            } else {
                return Err(anyhow::anyhow!("Could not read block at height {}", height));
            }
        } else {
            return Err(anyhow::anyhow!("Block at height {} not found in index", height));
        }
    }

    // Build Arrow arrays
    let arrays: Vec<Arc<dyn arrow::array::Array>> = builders.into_iter()
        .map(|mut builder| Arc::new(builder.finish()) as Arc<dyn arrow::array::Array>)
        .collect();

    // Create record batch
    let batch = RecordBatch::try_new(schema.clone(), arrays)?;

    // Write to Arrow file
    let file = File::create(&filename)?;
    let mut writer = FileWriter::try_new(file, &schema)?;
    writer.write(&batch)?;
    writer.close()?;

    println!("Successfully exported {} rows to {}", processed_count, filename.display());
    Ok(())
}

