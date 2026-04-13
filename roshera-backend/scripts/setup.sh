#!/bin/bash
set -e

echo "🔨 Setting up Roshera CAD Backend Development Environment"
echo "========================================================"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
NC='\033[0m'

# Check Rust installation
echo -n "Checking Rust installation... "
if command -v cargo >/dev/null 2>&1; then
    RUST_VERSION=$(rustc --version | cut -d' ' -f2)
    echo -e "${GREEN}✓ Found Rust $RUST_VERSION${NC}"
else
    echo -e "${RED}✗ Not found${NC}"
    echo "Please install Rust from https://rustup.rs/"
    exit 1
fi

# Check for required Rust version
REQUIRED_VERSION="1.70.0"
if [ "$(printf '%s\n' "$REQUIRED_VERSION" "$RUST_VERSION" | sort -V | head -n1)" != "$REQUIRED_VERSION" ]; then
    echo -e "${YELLOW}⚠️  Rust version $RUST_VERSION is older than recommended $REQUIRED_VERSION${NC}"
fi

# Install useful cargo extensions
echo ""
echo "Installing useful development tools..."

install_tool() {
    local tool=$1
    local package=${2:-$tool}
    
    echo -n "  $tool... "
    if command -v $tool >/dev/null 2>&1; then
        echo -e "${GREEN}already installed${NC}"
    else
        if cargo install $package; then
            echo -e "${GREEN}installed${NC}"
        else
            echo -e "${YELLOW}failed (optional)${NC}"
        fi
    fi
}

install_tool cargo-watch
install_tool cargo-audit
install_tool cargo-tarpaulin
install_tool cargo-criterion
install_tool cargo-edit

# Create necessary directories
echo ""
echo "Creating directories..."
mkdir -p exports logs models coverage

# Set up git hooks (if in git repo)
if [ -d .git ]; then
    echo ""
    echo "Setting up git hooks..."
    
    # Pre-commit hook for formatting and linting
    cat > .git/hooks/pre-commit << 'HOOK'
#!/bin/bash
echo "Running pre-commit checks..."

# Format check
if ! cargo fmt --all -- --check; then
    echo "❌ Code needs formatting. Run: cargo fmt --all"
    exit 1
fi

# Clippy check
if ! cargo clippy --workspace -- -D warnings; then
    echo "❌ Clippy found issues"
    exit 1
fi

echo "✅ Pre-commit checks passed"
HOOK
    
    chmod +x .git/hooks/pre-commit
    echo -e "${GREEN}✓ Git hooks configured${NC}"
fi

# Check for .env file
if [ ! -f .env ]; then
    echo ""
    echo -e "${YELLOW}⚠️  No .env file found, creating from template...${NC}"
    if [ -f .env.example ]; then
        cp .env.example .env
    else
        echo "RUST_LOG=info" > .env
    fi
fi

# Set permissions
echo ""
echo "Setting permissions..."
chmod +x scripts/*.sh

# Final instructions
echo ""
echo -e "${GREEN}✅ Setup complete!${NC}"
echo ""
echo "Next steps:"
echo "  1. Review and edit .env file for your environment"
echo "  2. Run './roshera-backend/scripts/dev.sh' to start development server"
echo "  3. Run './roshera-backend/scripts/test.sh' to run all tests"
echo ""
echo "Optional:"
echo "  - Install VS Code Rust extension: rust-analyzer"
echo "  - Install debugging tools: cargo install cargo-expand cargo-tree"
echo ""
