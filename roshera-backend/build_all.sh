#!/bin/bash
set -e
echo "🔨 Building Roshera CAD Backend..."
cd roshera-backend
cargo build --workspace
echo "✅ Build complete!"
echo ""
echo "To run the server:"
echo "  cargo run --bin api-server"
