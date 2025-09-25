# Bitcoin Fee Explorer - CLAUDE.md

## Project Overview

A Bitcoin blockchain indexer and analyzer with two components:
1. **CLI Indexer**: Fast Rust tool that reads Bitcoin block files directly and builds a height-based index
2. **Web Frontend**: Client-side visualization using WASM + Plotly.js (future integration)

## Architecture

### Current Focus: CLI Indexer
- **Block Reading**: Direct `.blk*.dat` file parsing with XOR deobfuscation
- **Smart Indexing**: Resolves blockchain forks to build height-ordered index
- **Fast Iteration**: Efficient block-by-block processing from any height range
- **No Database Dependencies**: Avoids complex LevelDB integration

### Future: Web Frontend
- **Frontend**: HTML/CSS/JS with Plotly.js for interactive charts
- **Data Processing**: Rust WASM module for analytics
- **Data Format**: Generated from CLI indexer output
- **Hosting**: Static files

## Current Status

✅ **CLI Indexer (Completed):**
- Direct Bitcoin block file reading (`.blk*.dat`)
- XOR deobfuscation support (`xor.dat`)
- Blockchain fork resolution and height calculation
- Height-based block index with tip detection
- Efficient block iteration in any height range
- Fee calculation and transaction counting
- Clean two-phase height calculation algorithm

✅ **Web Frontend (Previous Work):**
- WASM build pipeline with working demo
- Plotly.js charts with zoom/pan
- Mobile responsive design
- Mock data integration

## Project Structure

```
fee-explorer/
├── Cargo.toml                  # Rust dependencies
├── blockchain.idx              # Generated block index (binary)
├── src/
│   ├── lib.rs                  # WASM module (for frontend)
│   └── bin/
│       ├── main.rs             # CLI indexer main
│       ├── index.rs            # Height->location mapping
│       └── block_parser.rs     # Block file reader
├── www/                        # Frontend (previous work)
│   ├── index.html
│   ├── script.js
│   └── style.css
└── scripts/build.sh            # WASM build script
```

## CLI Indexer Usage

### Build the Indexer
```bash
cargo build --bin main
```

### Build Index from Bitcoin Data
```bash
# Uses ~/.bitcoin by default
cargo run --bin main -- build-index

# Or specify custom path (parent of blocks/ folder)
cargo run --bin main -- build-index --datadir /path/to/bitcoin
```

### Iterate Through Blocks
```bash
# Iterate from tip down to genesis
cargo run --bin main -- iterate

# Iterate specific range
cargo run --bin main -- iterate --start-height 800000 --end-height 799000

# Custom data directory
cargo run --bin main -- iterate --datadir /custom/path
```

### Example Output
```
Building index from data directory: /home/user/.bitcoin
✓ Found 1234 block files
Processing block files...
Found genesis block: 000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f
Calculating block heights...
Tip height: 820450
Built index for 820451 blocks
Index saved to: blockchain.idx

# Then iterating:
Height: 820450, Transactions: 3247, Fees: 0.15234567 BTC
Height: 820449, Transactions: 2891, Fees: 0.12847392 BTC
```

## Key Technical Features

### 1. Direct Block File Reading
- Reads `blocks/blk*.dat` files without Bitcoin Core databases
- Handles XOR obfuscation via `blocks/xor.dat`
- Robust magic byte validation and error recovery

### 2. Smart Blockchain Resolution
- **Two-phase height calculation**:
  1. Build stack backwards until finding known height
  2. Unwind stack forwards, assigning incremental heights
- Handles blockchain forks and orphaned blocks
- Finds longest valid chain automatically

### 3. Efficient Index Structure
```rust
pub struct BlockIndex {
    pub blocks: HashMap<u32, BlockLocation>, // height -> location
    pub tip_height: u32,
}

pub struct BlockLocation {
    pub file_path: String,     // which blk*.dat file
    pub file_offset: u64,      // offset within file
    pub block_hash: BlockHash, // block identifier
}
```

### 4. Fee Calculation
- Calculates exact block rewards per height (handles halvings)
- Computes fees as: `coinbase_outputs - block_reward`
- Supports transaction counting and basic statistics

## Configuration

### Default Paths
- **Data Directory**: `~/.bitcoin` (expandable tilde)
- **Block Files**: `{datadir}/blocks/blk*.dat`
- **XOR Key**: `{datadir}/blocks/xor.dat`
- **Index Output**: `./blockchain.idx`

### XOR Deobfuscation
Automatically loads 8-byte XOR key from `blocks/xor.dat`:
```rust
let xor_key = load_xor_key(&datadir)?;
// Applies: data[i] ^= xor_key[i % 8]
```

## Build & Development

### CLI Indexer
```bash
# Build
cargo build --bin main

# Build index (takes time for full blockchain)
cargo run --bin main -- build-index --datadir ~/.bitcoin

# Test iteration
cargo run --bin main -- iterate --start-height 100 --end-height 90
```

### Web Frontend (Previous Work)
```bash
# Build WASM
./scripts/build.sh

# Serve locally
python3 -m http.server 8000 --directory www
```

## Technical Decisions

### Why Direct Block Files vs LevelDB?
- **Simpler**: No complex database dependencies or Bitcoin Core internals
- **Portable**: Works with any Bitcoin node data directory
- **Reliable**: Avoids XOR obfuscation key issues and database format changes
- **Fast**: Direct file access without database overhead

### Two-Phase Height Calculation
Previous approach mixed stack building with height assignment. New approach:
1. **Build Stack**: Traverse backwards until finding known state
2. **Unwind Stack**: Process forwards, assigning heights incrementally

Benefits: clearer logic, better error handling, easier debugging.

## Performance Notes

### Index Building
- **Memory Usage**: ~50MB for full blockchain (height->location mapping)
- **Build Time**: ~10-30 minutes for full blockchain (depends on disk speed)
- **Output Size**: ~100MB binary index file

### Block Iteration
- **Speed**: ~1000 blocks/second on modern SSD
- **Memory**: Minimal (reads one block at a time)
- **Range Queries**: Efficient random access by height

## Integration Roadmap

### Phase 1: Data Export (Next)
1. Add CSV export to CLI indexer:
   ```bash
   cargo run --bin main -- iterate --output fees.csv --format csv
   ```

2. Export structured data for frontend:
   - Block height, timestamp, tx_count, fees
   - Fee rate statistics, block size, difficulty
   - Support time ranges for manageable datasets

### Phase 2: Frontend Integration
1. Load CSV data into frontend charts
2. Replace mock data with real blockchain statistics
3. Add metric selection for different data views

### Phase 3: Advanced Analytics
1. Moving averages and trend analysis
2. Mempool prediction and fee estimation
3. Network health metrics and correlation analysis

## Commands to Remember

```bash
# Build indexer
cargo build --bin main

# Index blockchain (one-time, slow)
cargo run --bin main -- build-index

# Quick test iteration
cargo run --bin main -- iterate --start-height 10 --end-height 0

# Help
cargo run --bin main -- --help
cargo run --bin main -- build-index --help
```

## Key Files for Next Session

1. **`src/bin/main.rs`** - CLI main logic, add CSV export
2. **`src/bin/index.rs`** - Index structure and serialization
3. **`src/bin/block_parser.rs`** - Block file reading with XOR
4. **`blockchain.idx`** - Generated index file (reusable)

The CLI indexer is working and efficient! Ready for data export and frontend integration.