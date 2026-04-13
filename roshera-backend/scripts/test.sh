#!/bin/bash
set -e

echo "🧪 Running Comprehensive Test Suite"
echo "==================================="

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Function to run tests for a crate
run_crate_tests() {
    local crate=$1
    echo -e "\n${YELLOW}Testing $crate...${NC}"
    
    if cargo test --package $crate --lib --tests; then
        echo -e "${GREEN}✅ $crate tests passed${NC}"
    else
        echo -e "${RED}❌ $crate tests failed${NC}"
        exit 1
    fi
}

# Unit tests for each crate
for crate in shared-types geometry-engine session-manager api-server ai-integration export-engine; do
    run_crate_tests $crate
done

# Integration tests
echo -e "\n${YELLOW}Running integration tests...${NC}"
if cargo test --package integration-tests; then
    echo -e "${GREEN}✅ Integration tests passed${NC}"
else
    echo -e "${RED}❌ Integration tests failed${NC}"
    exit 1
fi

# Documentation tests
echo -e "\n${YELLOW}Running documentation tests...${NC}"
if cargo test --workspace --doc; then
    echo -e "${GREEN}✅ Documentation tests passed${NC}"
else
    echo -e "${RED}❌ Documentation tests failed${NC}"
    exit 1
fi

# Clippy linting
echo -e "\n${YELLOW}Running Clippy lints...${NC}"
if cargo clippy --workspace --all-targets -- -D warnings; then
    echo -e "${GREEN}✅ Clippy checks passed${NC}"
else
    echo -e "${RED}❌ Clippy found issues${NC}"
    exit 1
fi

# Format check
echo -e "\n${YELLOW}Checking code formatting...${NC}"
if cargo fmt --all -- --check; then
    echo -e "${GREEN}✅ Code formatting correct${NC}"
else
    echo -e "${RED}❌ Code needs formatting (run: cargo fmt --all)${NC}"
    exit 1
fi

# Security audit
echo -e "\n${YELLOW}Running security audit...${NC}"
if command -v cargo-audit >/dev/null 2>&1; then
    if cargo audit; then
        echo -e "${GREEN}✅ No security vulnerabilities found${NC}"
    else
        echo -e "${YELLOW}⚠️  Security vulnerabilities detected${NC}"
    fi
else
    echo -e "${YELLOW}⚠️  cargo-audit not installed, skipping security check${NC}"
fi

# Test coverage (if tarpaulin is installed)
if command -v cargo-tarpaulin >/dev/null 2>&1; then
    echo -e "\n${YELLOW}Generating test coverage report...${NC}"
    cargo tarpaulin --workspace --out Html --output-dir coverage
    echo -e "${GREEN}✅ Coverage report generated in coverage/tarpaulin-report.html${NC}"
fi

echo -e "\n${GREEN}🎉 All tests passed!${NC}"
