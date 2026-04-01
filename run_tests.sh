#!/bin/bash
# Test suite runner for brainjar
set -e

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "🧪 Running brainjar test suite"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

echo ""
echo "📦 Building project..."
cargo build --quiet

echo ""
echo "🔬 Running unit tests..."
cargo test --lib --quiet

echo ""
echo "🧩 Running integration tests..."
cargo test --test '*' --quiet

echo ""
echo "✅ All tests passed!"
echo ""
echo "📊 Test coverage summary:"
cargo test --lib -- --list 2>/dev/null | grep -E '^    ' | wc -l | xargs echo "  Unit tests:"
cargo test --test '*' -- --list 2>/dev/null | grep -E '^    ' | wc -l | xargs echo "  Integration tests:"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
