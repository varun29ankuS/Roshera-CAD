#!/bin/bash
set -e

echo "⚡ Running Performance Benchmarks"
echo "================================="

# Check if criterion is available
if ! grep -q "criterion" geometry-engine/Cargo.toml; then
    echo "Adding criterion for benchmarks..."
    cd geometry-engine
    cargo add --dev criterion
    cd ..
fi

# Run benchmarks for geometry engine
echo "🔧 Benchmarking geometry operations..."
cargo bench --package geometry-engine

# Create benchmark report
if [ -d "target/criterion" ]; then
    echo ""
    echo "📊 Benchmark Results Summary:"
    echo "============================"
    
    # Parse and display key metrics
    find target/criterion -name "*.json" -type f | while read -r file; do
        if command -v jq >/dev/null 2>&1; then
            name=$(jq -r '.title' "$file" 2>/dev/null || echo "Unknown")
            mean=$(jq -r '.mean.point_estimate' "$file" 2>/dev/null || echo "N/A")
            if [ "$name" != "Unknown" ] && [ "$mean" != "N/A" ]; then
                echo "  $name: ${mean}ns"
            fi
        fi
    done
    
    echo ""
    echo "📈 Detailed reports available in target/criterion/report/index.html"
fi

echo ""
echo "✅ Benchmarks complete!"
