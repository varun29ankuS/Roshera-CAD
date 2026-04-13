#!/bin/bash
set -e

echo "🚀 Starting Roshera CAD Backend Development Environment"
echo "======================================================"

# Check dependencies
command -v cargo >/dev/null 2>&1 || { echo "❌ Rust/Cargo not installed. Please install from https://rustup.rs/"; exit 1; }

# Create necessary directories
mkdir -p exports logs

# Check for .env file
if [ ! -f .env ]; then
    echo "⚠️  .env file not found, using defaults"
fi

# Build all crates
echo "🔨 Building all crates..."
cargo build --workspace || { echo "❌ Build failed"; exit 1; }

# Start API server with auto-reload
echo "🌐 Starting API server on http://localhost:8080"
echo "📊 Metrics available on http://localhost:9090"
echo ""
echo "Available endpoints:"
echo "  GET  http://localhost:8080/health"
echo "  POST http://localhost:8080/api/geometry"
echo "  POST http://localhost:8080/api/boolean"
echo "  POST http://localhost:8080/api/ai/command"
echo "  POST http://localhost:8080/api/export"
echo "  WS   ws://localhost:8080/ws/{session_id}"
echo ""
echo "Press Ctrl+C to stop"
echo ""

# Run with cargo watch if available, otherwise direct run
if command -v cargo-watch >/dev/null 2>&1; then
    cargo watch -x "run --bin api-server"
else
    cargo run --bin api-server
fi
