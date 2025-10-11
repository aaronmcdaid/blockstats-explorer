```
cargo build --bin main --release && /usr/bin/time --verbose cargo run --bin main --release -- export www/data/complete_analysis.arrow height timestamp  op_return_count op_return_bytes op_return_gt40 op_return_gt80 tx_count fee_avg block_size tx_size[0,25,50,75,100]
```
