use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io;
use std::sync::Arc;
use bitcoin::BlockHash;
use bitcoin::hashes::Hash as BitcoinHash;
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
        #[arg(long, help = "Enable UTXO tracking for accurate per-transaction fee calculations")]
        utxo: bool,
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
        Commands::Export { datadir, filename, columns, max_height, utxo } => {
            let expanded_datadir = expand_tilde(&datadir);
            println!("Exporting columns {:?} to {}", columns, filename.display());
            if utxo {
                println!("🔍 UTXO tracking enabled for accurate fee calculations");
            }
            export_arrow_file(expanded_datadir, filename, columns, max_height, utxo)?;
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

fn utxo_key(txid: &bitcoin::Txid, output_index: u32) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(txid, &mut hasher);
    std::hash::Hash::hash(&output_index, &mut hasher);
    hasher.finish()
}

struct UtxoSet {
    active: HashMap<u64, u64>,
    to_remove: HashSet<u64>,
}

impl UtxoSet {
    fn new() -> Self {
        Self {
            active: HashMap::new(),
            to_remove: HashSet::new(),
        }
    }

    fn add_output(&mut self, txid: &bitcoin::Txid, output_index: u32, value_sats: u64, block_height: u32) -> anyhow::Result<()> {
        let key = utxo_key(txid, output_index);

        // Collision detection - error if key already exists (except blocks with duplicate coinbase)
        if self.active.contains_key(&key) && block_height != 91842 && block_height != 91880 {
            return Err(anyhow::anyhow!(
                "UTXO key collision detected at block {}: {}:{} (hash: {})",
                block_height, txid, output_index, key
            ));
        }

        self.active.insert(key, value_sats);
        Ok(())
    }

    fn mark_for_removal(&mut self, txid: &bitcoin::Txid, output_index: u32) {
        let key = utxo_key(txid, output_index);
        self.to_remove.insert(key);
    }

    fn get_value(&self, txid: &bitcoin::Txid, output_index: u32) -> Option<u64> {
        let key = utxo_key(txid, output_index);
        self.active.get(&key).copied()
    }

    fn commit_removals(&mut self) {
        for key in &self.to_remove {
            self.active.remove(key);
        }
        self.to_remove.clear();
    }

    fn len(&self) -> usize {
        self.active.len()
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
    let mut blocks_by_hash: HashMap<BlockHash, (u64, String, BlockHash, BlockHeight, u32)> = HashMap::new(); // hash -> (offset, file_path, prev_hash, height, block_size)
    let mut genesis_hash: Option<BlockHash> = None;

    for blk_file in &blk_files {
        println!("Processing file: {}", blk_file.display());
        let mut reader = BlockFileReader::new_with_xor_key(blk_file, xor_key)?;
        let mut block_count = 0;

        while let Some((header, offset, block_size)) = reader.read_next_header()? {
            let block_hash = header.block_hash();
            let prev_hash = header.prev_blockhash;

            blocks_by_hash.insert(
                block_hash,
                (offset, blk_file.to_string_lossy().to_string(), prev_hash, BlockHeight::NotYetKnown, block_size)
            );

            // Check if this is the genesis block (prev_hash is all zeros)
            if prev_hash == BlockHash::from_byte_array([0; 32]) {
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
            if let Some((_, _, _, height, _)) = blocks_by_hash.get_mut(&hash) {
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
        blocks_map: &mut HashMap<BlockHash, (u64, String, BlockHash, BlockHeight, u32)>
    ) -> anyhow::Result<BlockHeight> {
        // Check if height is already calculated
        if let Some((_, _, _, height, _)) = blocks_map.get(start_hash) {
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
            let (_, _, prev_hash, height, _) = match blocks_map.get(&current_hash) {
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
        .filter_map(|(_, _, _, height, _)| match height {
            BlockHeight::Known(h) => Some(*h),
            _ => None,
        })
        .max();

    let tip_hash = if let Some(max_height) = tip_height {
        // Find all blocks at tip height
        let tip_blocks: Vec<BlockHash> = blocks_by_hash.iter()
            .filter_map(|(hash, (_, _, _, height, _))| {
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
        if let Some((offset, file_path, prev_hash, _, block_size)) = blocks_by_hash.get(&current_hash) {
            let location = BlockLocation {
                file_path: file_path.clone(),
                file_offset: *offset,
                block_hash: current_hash,
                block_size: *block_size,
            };

            block_index.add_block(current_height, location);

            // Move to previous block
            if *prev_hash == BlockHash::from_byte_array([0; 32]) {
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
type ColumnExtractor = fn(&bitcoin::Block, u32, Option<&UtxoSet>) -> f64;
type MultiColumnExtractor = fn(&bitcoin::Block, u32, Option<&UtxoSet>) -> Vec<f64>;

#[derive(Debug, Clone)]
enum ColumnSpec {
    Single(String, ColumnExtractor),
    Multi(String, Vec<f64>, MultiColumnExtractor), // base_name, quantiles, extractor
}

fn parse_column_spec(column_input: &str) -> anyhow::Result<ColumnSpec> {
    // Check for quantile syntax: name[q1,q2,q3]
    if let Some(bracket_start) = column_input.find('[') {
        if !column_input.ends_with(']') {
            return Err(anyhow::anyhow!("Invalid quantile syntax: missing closing ']' in '{}'", column_input));
        }

        let base_name = column_input[..bracket_start].to_string();
        let quantiles_str = &column_input[bracket_start + 1..column_input.len() - 1];

        // Parse quantiles
        let quantiles: Result<Vec<f64>, _> = quantiles_str
            .split(',')
            .map(|s| s.trim().parse::<f64>())
            .collect();

        let quantiles = quantiles.map_err(|_| anyhow::anyhow!("Invalid quantile values in '{}'", column_input))?;

        // Validate quantiles are in 0-100 range
        for &q in &quantiles {
            if q < 0.0 || q > 100.0 {
                return Err(anyhow::anyhow!("Quantile {} out of range [0,100]", q));
            }
        }

        let extractor = get_multi_column_extractor(&base_name)?;
        Ok(ColumnSpec::Multi(base_name, quantiles, extractor))
    } else {
        // Regular single column
        let extractor = get_column_extractor(column_input)?;
        Ok(ColumnSpec::Single(column_input.to_string(), extractor))
    }
}

fn get_column_extractor(column_name: &str) -> anyhow::Result<ColumnExtractor> {
    match column_name {
        "height" => Ok(|_block, height, _utxo| height as f64),
        "timestamp" => Ok(|block, _height, _utxo| block.header.time as f64),
        "tx_count" => Ok(|block, _height, _utxo| block.txdata.len() as f64),
        "fee_avg" => Ok(|block, height, _utxo| {
            let fees = calculate_block_fees(&block.txdata, height);

            // Calculate total vBytes for non-coinbase transactions
            let total_vbytes: f64 = block.txdata.iter()
                .skip(1) // Skip coinbase
                .map(|tx| tx.weight().to_wu() as f64 / 4.0)
                .sum();

            if total_vbytes > 0.0 {
                fees.to_sat() as f64 / total_vbytes
            } else {
                0.0
            }
        }),
        "block_size" => Ok(|_block, _height, _utxo| {
            // Note: block_size is now cached in the index, this function won't be used for block_size
            // This is kept for compatibility, but the export function uses cached values
            0.0
        }),
        "utxo_size" => Ok(|_block, _height, utxo| {
            let utxo_set = utxo.expect("utxo_size requires UTXO data - this should have been caught by validation");
            utxo_set.len() as f64
        }),
        "op_return_count" => Ok(|block, _height, _utxo| {
            // Count total number of OP_RETURN outputs across all transactions in the block
            let mut op_return_count = 0;
            for tx in &block.txdata {
                for output in &tx.output {
                    if output.script_pubkey.is_op_return() {
                        op_return_count += 1;
                    }
                }
            }
            op_return_count as f64
        }),
        "op_return_bytes" => Ok(|block, _height, _utxo| {
            // Sum total bytes in all OP_RETURN outputs across all transactions in the block
            let mut total_op_return_bytes = 0;
            for tx in &block.txdata {
                for output in &tx.output {
                    if output.script_pubkey.is_op_return() {
                        total_op_return_bytes += output.script_pubkey.len();
                    }
                }
            }
            total_op_return_bytes as f64
        }),
        "op_return_gt40" => Ok(|block, _height, _utxo| {
            // Count OP_RETURN outputs larger than 40 bytes
            let mut count_gt40 = 0;
            for tx in &block.txdata {
                for output in &tx.output {
                    if output.script_pubkey.is_op_return() && output.script_pubkey.len() > 40 {
                        count_gt40 += 1;
                    }
                }
            }
            count_gt40 as f64
        }),
        "op_return_gt80" => Ok(|block, _height, _utxo| {
            // Count OP_RETURN outputs larger than 80 bytes
            let mut count_gt80 = 0;
            for tx in &block.txdata {
                for output in &tx.output {
                    if output.script_pubkey.is_op_return() && output.script_pubkey.len() > 80 {
                        count_gt80 += 1;
                    }
                }
            }
            count_gt80 as f64
        }),
        _ => Err(anyhow::anyhow!("Unknown column: {}", column_name)),
    }
}

fn column_requires_utxo(column_name: &str) -> bool {
    match column_name {
        "fee_rates" | "utxo_size" => true,
        _ => false,
    }
}

fn get_multi_column_extractor(base_name: &str) -> anyhow::Result<MultiColumnExtractor> {
    match base_name {
        "tx_size" => Ok(|block, _height, _utxo| {
            // Transaction sizes in vbytes
            let mut sizes: Vec<f64> = block.txdata.iter()
                .skip(1) // Skip coinbase
                .map(|tx| tx.weight().to_wu() as f64 / 4.0)
                .collect();

            sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
            sizes
        }),
        "fee_rates" => Ok(|block, _height, utxo| {
            let utxo_set = utxo.expect("fee_rates requires UTXO data - this should have been caught by validation");

            // Calculate fee rate for each non-coinbase transaction
            let mut fee_rates = Vec::new();

            for (tx_idx, tx) in block.txdata.iter().enumerate() {
                if tx_idx == 0 {
                    continue; // Skip coinbase
                }

                // Calculate input value (assume all inputs are in UTXO set)
                let mut input_value = 0u64;
                for input in &tx.input {
                    let value = utxo_set.get_value(&input.previous_output.txid, input.previous_output.vout)
                        .expect("Input not found in UTXO set");
                    input_value += value;
                }

                // Calculate output value
                let output_value: u64 = tx.output.iter()
                    .map(|output| output.value.to_sat())
                    .sum();

                // Assert that input >= output (no value creation)
                assert!(input_value >= output_value, "Input value {} < output value {} for transaction", input_value, output_value);

                // Calculate fee and fee rate
                let fee = input_value - output_value;
                let tx_vsize = tx.weight().to_wu() as f64 / 4.0;

                if tx_vsize > 0.0 {
                    fee_rates.push(fee as f64 / tx_vsize);
                }
            }

            fee_rates.sort_by(|a, b| a.partial_cmp(b).unwrap());
            fee_rates
        }),
        _ => Err(anyhow::anyhow!("Unknown multi-column: {}", base_name)),
    }
}

fn calculate_quantiles(sorted_data: &[f64], quantiles: &[f64]) -> Vec<f64> {
    if sorted_data.is_empty() {
        return vec![0.0; quantiles.len()];
    }

    quantiles.iter().map(|&q| {
        if q == 0.0 {
            sorted_data[0]
        } else if q == 100.0 {
            sorted_data[sorted_data.len() - 1]
        } else {
            let pos = (q / 100.0) * (sorted_data.len() - 1) as f64;
            let lower = pos.floor() as usize;
            let upper = pos.ceil() as usize;

            if lower == upper {
                sorted_data[lower]
            } else {
                let weight = pos - lower as f64;
                sorted_data[lower] * (1.0 - weight) + sorted_data[upper] * weight
            }
        }
    }).collect()
}

fn export_arrow_file(
    datadir: PathBuf,
    filename: PathBuf,
    columns: Vec<String>,
    max_height: Option<u32>,
    utxo: bool,
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

    // Parse column specifications
    let mut column_specs = Vec::new();
    let mut expanded_column_names = Vec::new();

    for column_input in &columns {
        let spec = parse_column_spec(column_input)?;
        match &spec {
            ColumnSpec::Single(name, _) => {
                expanded_column_names.push(name.clone());
            }
            ColumnSpec::Multi(base_name, quantiles, _) => {
                for &q in quantiles {
                    expanded_column_names.push(format!("{}_{}", base_name, q as u32));
                }
            }
        }
        column_specs.push(spec);
    }

    // Validate that columns requiring UTXO data have the --utxo flag
    if !utxo {
        for column_input in &columns {
            let base_name = column_input.split('[').next().unwrap_or(column_input);
            if column_requires_utxo(base_name) {
                return Err(anyhow::anyhow!(
                    "Column '{}' requires UTXO tracking for accurate calculations.\n\
                     Please add the --utxo flag to enable UTXO tracking:\n\
                     \n\
                     cargo run --bin main -- export {} --utxo [other options]\n\
                     \n\
                     Note: UTXO tracking uses more memory but provides accurate per-transaction data.",
                    column_input,
                    filename.display()
                ));
            }
        }
    }

    println!("Exporting {} columns (expanded to {} columns) from height {} to {}",
             columns.len(), expanded_column_names.len(), export_min_height, export_max_height);

    // Create Arrow schema using expanded column names
    let fields: Vec<Field> = expanded_column_names.iter()
        .map(|name| Field::new(name, DataType::Float64, false))
        .collect();
    let schema = Arc::new(Schema::new(fields));

    // Create builders for each expanded column
    let mut builders: Vec<Float64Builder> = expanded_column_names.iter()
        .map(|_| Float64Builder::new())
        .collect();

    // Initialize UTXO set if needed
    let mut utxo_set = if utxo {
        Some(UtxoSet::new())
    } else {
        None
    };

    // Process blocks and collect data
    let mut processed_count = 0;
    for height in export_min_height..=export_max_height {
        if let Some(location) = block_index.blocks.get(&height) {
            let mut reader = BlockFileReader::new_with_xor_key(&location.file_path, xor_key)?;
            reader.seek_to_offset(location.file_offset)?;

            if let Some((block, _offset)) = reader.read_next_block()? {
                // UTXO tracking: Add block outputs to UTXO set
                if let Some(ref mut utxo) = utxo_set {
                    for (_tx_idx, tx) in block.txdata.iter().enumerate() {
                        let txid = tx.txid();
                        for (output_idx, output) in tx.output.iter().enumerate() {
                            // Skip OP_RETURN outputs (provably unspendable)
                            if output.script_pubkey.is_op_return() {
                                continue;
                            }
                            utxo.add_output(&txid, output_idx as u32, output.value.to_sat(), height)?;
                        }
                    }
                }

                // Extract values for each column spec
                let mut builder_idx = 0;

                for spec in &column_specs {
                    match spec {
                        ColumnSpec::Single(name, extractor) => {
                            let value = if name == "block_size" {
                                // Use cached block size from index
                                location.block_size as f64
                            } else {
                                // Use extractor function
                                extractor(&block, height, utxo_set.as_ref())
                            };
                            builders[builder_idx].append_value(value);
                            builder_idx += 1;
                        }
                        ColumnSpec::Multi(_, quantiles, extractor) => {
                            // Extract all values and calculate quantiles
                            let data = extractor(&block, height, utxo_set.as_ref());
                            let quantile_values = calculate_quantiles(&data, quantiles);

                            // Append each quantile value to its respective builder
                            for value in quantile_values {
                                builders[builder_idx].append_value(value);
                                builder_idx += 1;
                            }
                        }
                    }
                }

                // UTXO tracking: Mark block inputs for removal and commit
                if let Some(ref mut utxo) = utxo_set {
                    for (tx_idx, tx) in block.txdata.iter().enumerate() {
                        if tx_idx == 0 {
                            continue; // Skip coinbase (no inputs to spend)
                        }
                        for input in &tx.input {
                            utxo.mark_for_removal(&input.previous_output.txid, input.previous_output.vout);
                        }
                    }
                    utxo.commit_removals();

                    // Log UTXO set size periodically
                    if processed_count % 1000 == 0 && processed_count > 0 {
                        println!("Block {}: UTXO set size: {} UTXOs", processed_count, utxo.len());
                    }
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

    // Immediately verify the exported file by reopening it
    println!("\n🔍 Verifying exported Arrow file...");
    verify_arrow_file(&filename)?;

    Ok(())
}

fn verify_arrow_file(filename: &PathBuf) -> anyhow::Result<()> {
    use arrow_ipc::reader::FileReader;
    use std::fs::File;

    let file = File::open(filename)?;
    let reader = FileReader::try_new(file, None)?;

    let schema = reader.schema();
    let total_rows: usize = reader.into_iter().map(|batch| batch.unwrap().num_rows()).sum();

    println!("✅ Arrow file verification successful!");
    println!("📊 Schema Summary:");
    println!("   - Total columns: {}", schema.fields().len());
    println!("   - Total rows: {}", total_rows);
    println!("   - File size: {} bytes", std::fs::metadata(filename)?.len());

    println!("\n📋 Column Details:");
    for (i, field) in schema.fields().iter().enumerate() {
        println!("   {}. {} ({})",
                 i + 1,
                 field.name(),
                 format_data_type(field.data_type()));
    }

    // Show first few values from each column for a quick data preview
    let file = File::open(filename)?;
    let mut reader = FileReader::try_new(file, None)?;

    if let Some(Ok(first_batch)) = reader.next() {
        if first_batch.num_rows() > 0 {
            println!("\n📈 Sample Data (first row):");
            for (i, array) in first_batch.columns().iter().enumerate() {
                let field = schema.field(i);
                let sample_value = format_array_value(array, 0);
                println!("   {}: {}", field.name(), sample_value);
            }
        }
    }

    Ok(())
}

fn format_data_type(data_type: &arrow::datatypes::DataType) -> &'static str {
    match data_type {
        arrow::datatypes::DataType::Float64 => "Float64",
        arrow::datatypes::DataType::Int32 => "Int32",
        arrow::datatypes::DataType::Int64 => "Int64",
        arrow::datatypes::DataType::Utf8 => "String",
        _ => "Other",
    }
}

fn format_array_value(array: &Arc<dyn arrow::array::Array>, index: usize) -> String {
    use arrow::array::Array;

    if array.is_null(index) {
        return "null".to_string();
    }

    // Try to cast to Float64Array since that's what we use
    if let Some(float_array) = array.as_any().downcast_ref::<arrow::array::Float64Array>() {
        format!("{:.2}", float_array.value(index))
    } else {
        "unknown".to_string()
    }
}

