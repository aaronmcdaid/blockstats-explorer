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
- Apache Arrow export with quantile statistics
- Multi-column export support (e.g., `fees[0,50,100]`)

✅ **Web Frontend (Completed):**
- WASM build pipeline with working demo
- Plotly.js charts with zoom/pan
- Mobile responsive design
- Real Arrow data integration
- Dynamic block range discovery
- Multi-axis charting with proper unit grouping

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
├── www/                        # Frontend
│   ├── index.html
│   ├── script.js
│   ├── style.css
│   ├── data/
│   │   ├── datasets.json       # Metadata for Arrow files
│   │   └── complete_analysis.arrow # Exported blockchain data
│   └── pkg/                    # Generated WASM files
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

### Export Arrow Data for Frontend
```bash
# Export all available columns (recommended)
cargo run --bin main -- export complete_analysis.arrow height timestamp tx_count fee_avg block_size fees[0,25,50,75,100] tx_size[0,25,50,75,100]

# Export specific metrics only
cargo run --bin main -- export fees_only.arrow height fee_avg fees[0,50,100]

# Export with height limit
cargo run --bin main -- export recent.arrow height tx_count fee_avg --max-height 1000
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
    pub block_size: u32,       // size in bytes
}
```

### 4. Advanced Analytics
- Calculates exact block rewards per height (handles halvings)
- Computes fees as: `coinbase_outputs - block_reward`
- **Quantile Statistics**: Multi-percentile analysis with `fees[0,25,50,75,100]` syntax
- **Column Specs**: Single columns (`tx_count`) and multi-columns (`fees[...]`)
- **Arrow Export**: Efficient columnar format for large datasets
- **Schema Validation**: Automatic verification of exported data structure

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

### Web Frontend
```bash
# Build WASM
./scripts/build.sh

# Export data for frontend (place in www/data/)
cargo run --bin main -- export www/data/complete_analysis.arrow height timestamp tx_count fee_avg block_size fees[0,25,50,75,100] tx_size[0,25,50,75,100]

# Serve locally
python3 -m http.server 8000 --directory www

# Open in browser
open http://localhost:8000
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

## Web Frontend Features

### ✅ Completed
- **Dynamic Data Loading**: Automatically loads Arrow files and discovers block range
- **Interactive Charts**: Plotly.js with zoom, pan, hover, and reset controls
- **Multi-Metric Selection**: Choose from 15+ available blockchain metrics
- **Quantile Analysis**: Fee rate and transaction size percentiles (0th, 25th, 50th, 75th, 100th)
- **Dual Y-Axis**: Smart grouping by units (sat/vB, bytes, transactions on left; difficulty on right)
- **Moving Averages**: Configurable window size with dashed line overlay
- **Log Scale**: Toggle logarithmic scaling for better visualization of wide ranges
- **Responsive Design**: Works on desktop and mobile devices

### Key Frontend Files
- **`www/script.js`**: Main application logic, Arrow loading, chart generation
- **`www/data/datasets.json`**: Schema metadata (automatically updated with discovered block range)
- **`www/data/complete_analysis.arrow`**: Real blockchain data (generated by CLI export)
- **`www/index.html`**: UI with metric selection, range controls, chart options

### Future Enhancements
1. **Time-based Analysis**: Date range selection instead of block heights
2. **Advanced Statistics**: Correlation analysis between metrics
3. **Export Features**: Download charts as images, export filtered data
4. **Performance**: Lazy loading for very large datasets
5. **Additional Metrics**: Mempool data, network difficulty trends

## Commands to Remember

```bash
# Build indexer
cargo build --bin main

# Index blockchain (one-time, slow)
cargo run --bin main -- build-index

# Export data for frontend (complete dataset)
cargo run --bin main -- export www/data/complete_analysis.arrow height timestamp tx_count fee_avg block_size fees[0,25,50,75,100] tx_size[0,25,50,75,100]

# Quick test iteration
cargo run --bin main -- iterate --start-height 10 --end-height 0

# Start web server
python3 -m http.server 8000 --directory www

# Help
cargo run --bin main -- --help
cargo run --bin main -- export --help
```

## Key Files

1. **`src/bin/main.rs`** - CLI with indexing, iteration, and Arrow export commands
2. **`src/bin/index.rs`** - Index structure with height->location+size mapping
3. **`src/bin/block_parser.rs`** - Block file reading with XOR deobfuscation
4. **`www/script.js`** - Frontend with dynamic Arrow loading and interactive charts
5. **`www/data/datasets.json`** - Schema metadata (block range auto-discovered)
6. **`blockchain.idx`** - Generated index file (reusable across sessions)

## Project Status: Complete MVP

✅ **Full Pipeline Working:**
- CLI builds blockchain index from block files
- Export command generates Arrow files with quantile statistics
- Web frontend loads Arrow data and renders interactive charts
- Dynamic block range discovery from actual data
- Multi-metric visualization with proper axis grouping

The Bitcoin Fee Explorer is now a complete working system for analyzing blockchain fee markets!