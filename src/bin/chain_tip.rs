use std::path::PathBuf;
use clap::Parser;
use anyhow::{Result, Context, bail};
use bitcoin::BlockHash;
use bitcoin_hashes::Hash;
use hex;
use rusty_leveldb::{DB, Options};

fn format_hex(data: &[u8]) -> String {
    let hex_str = hex::encode(data);
    let mut formatted = String::new();
    for (i, c) in hex_str.chars().enumerate() {
        if i > 0 && i % 8 == 0 {
            formatted.push(' ');
        }
        formatted.push(c);
    }
    formatted
}

#[derive(Parser)]
#[command(name = "chain-tip")]
#[command(about = "Read Bitcoin chainstate and print tip information")]
struct Args {
    /// Path to Bitcoin data directory
    #[arg(short, long)]
    datadir: PathBuf,
}

fn read_obfuscation_key(db: &mut DB) -> Result<Vec<u8>> {
    // The obfuscation key is stored under key [0x0e, 'o', 'b', 'f', 'u', 's', 'c', 'a', 't', 'e', '_', 'k', 'e', 'y']
    let key = b"\x0e\x00obfuscate_key";

    match db.get(key) {
        Some(value) => {
            let mut vec = value.to_vec();
            assert!(vec.len() >= 9, "Obfuscation key value must be at least 9 bytes long");
            vec.remove(0);  // Drop the first byte
            Ok(vec)
        }
        None => {
            bail!("Obfuscation key not found in chainstate database - this indicates a corrupted or very old database");
        }
    }
}

fn deobfuscate_value(data: &[u8], obfuscation_key: &[u8]) -> Vec<u8> {
    if obfuscation_key.iter().all(|&b| b == 0) {
        // No obfuscation
        return data.to_vec();
    }

    let mut result = Vec::with_capacity(data.len());
    for (i, &byte) in data.iter().enumerate() {
        let key_byte = obfuscation_key[i % obfuscation_key.len()];
        result.push(byte ^ key_byte);
    }
    result
}

fn read_best_block_hash(db: &mut DB, obfuscation_key: &[u8]) -> Result<BlockHash> {
    // The best block hash is stored under key 'B'
    let key = b"B";

    match db.get(key) {
        Some(obfuscated_value) => {
            let value = deobfuscate_value(&obfuscated_value, obfuscation_key);

            if value.len() < 32 {
                bail!("Best block hash value too short: {} bytes", value.len());
            }

            // First 32 bytes should be the block hash
            let hash_bytes: [u8; 32] = value[0..32].try_into()
                .context("Failed to extract hash bytes")?;

            Ok(BlockHash::from_byte_array(hash_bytes))
        }
        None => {
            bail!("Best block hash ('B' key) not found in chainstate");
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Reading Bitcoin chainstate from: {:?}", args.datadir);

    // Check if chainstate directory exists
    let chainstate_path = args.datadir.join("chainstate");
    if !chainstate_path.exists() {
        bail!("Chainstate directory not found at {:?}. Is this a valid Bitcoin datadir?", chainstate_path);
    }

    println!("✓ Chainstate directory found: {:?}", chainstate_path);

    // Open chainstate database
    let mut options = Options::default();
    options.create_if_missing = false;

    let mut db = DB::open(&chainstate_path, options)
        .context("Failed to open chainstate database")?;

    println!("✓ Chainstate database opened successfully");

    // Step 1: Read and display obfuscation key
    println!("\n🔑 STEP 1: Reading obfuscation key");
    let obfuscation_key = read_obfuscation_key(&mut db)
        .context("Failed to read obfuscation key")?;

    println!("Obfuscation key (hex): {}", format_hex(&obfuscation_key));
    if obfuscation_key.iter().all(|&b| b == 0) {
        println!("Note: All zeros means no obfuscation");
    }

    // Step 2: Read raw (obfuscated) tip data
    println!("\n📍 STEP 2: Reading raw tip data");
    let tip_key = b"B";

    match db.get(tip_key) {
        Some(raw_tip_data) => {
            println!("Raw tip data (hex): {}", format_hex(&raw_tip_data));
            println!("Raw tip data length: {} bytes", raw_tip_data.len());

            // Step 3: Deobfuscate and show the result
            println!("\n🔓 STEP 3: Deobfuscating tip data");
            let deobfuscated = deobfuscate_value(&raw_tip_data, &obfuscation_key);
            println!("Deobfuscated data (hex): {}", format_hex(&deobfuscated));
            println!("Deobfuscated length: {} bytes", deobfuscated.len());

            // Step 4: Try to extract the block hash
            println!("\n🎯 STEP 4: Extracting block hash");
            if deobfuscated.len() >= 32 {
                let hash_bytes = &deobfuscated[0..32];
                println!("First 32 bytes (block hash): {}", format_hex(hash_bytes));

                // Try to create BlockHash
                if let Ok(hash_array) = hash_bytes.try_into() {
                    let block_hash = BlockHash::from_byte_array(hash_array);
                    println!("Parsed as BlockHash: {}", block_hash);
                } else {
                    println!("Failed to convert to BlockHash");
                }

                // Show remaining bytes if any
                if deobfuscated.len() > 32 {
                    let remaining = &deobfuscated[32..];
                    println!("Remaining {} bytes: {}", remaining.len(), format_hex(remaining));
                }
            } else {
                println!("ERROR: Deobfuscated data too short for block hash (need 32 bytes, got {})", deobfuscated.len());
            }
        }
        None => {
            println!("ERROR: 'B' key not found in chainstate database");
        }
    }

    Ok(())
}
