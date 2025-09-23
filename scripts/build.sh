#!/bin/bash

set -e

echo "Building Bitcoin Fee Explorer..."

# Build WASM module
echo "Building WASM module..."
~/.cargo/bin/wasm-pack build --target web --out-dir pkg

# Copy WASM files to www directory
echo "Copying WASM files to www..."
mkdir -p www/pkg
cp pkg/fee_explorer.js www/pkg/
cp pkg/fee_explorer_bg.wasm www/pkg/
cp pkg/fee_explorer.d.ts www/pkg/

echo "Build complete!"
echo ""
echo "For local development, run one of:"
echo "  python3 -m http.server 8000 --directory www"
echo "  php -S localhost:8000 -t www"
echo "  npx serve www"
echo ""
echo "Then open http://localhost:8000"