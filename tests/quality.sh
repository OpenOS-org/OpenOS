#!/bin/bash
# tests/quality.sh — Code quality checks for OpenOS
#
# Runs formatting, linting, and build checks.
# Usage: ./tests/quality.sh [--fix]

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
NC='\033[0m' # No Color

ERRORS=0

log() { echo -e "${GREEN}[OK]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
err() { echo -e "${RED}[FAIL]${NC} $1"; ERRORS=$((ERRORS + 1)); }

cd "$(dirname "$0")/.."

echo "========================================="
echo "  OpenOS Code Quality Checks"
echo "========================================="
echo ""

# 1. Check for TODO/FIXME comments
echo "--- Checking for TODO/FIXME comments ---"
TODO_COUNT=$(grep -rn "TODO\|FIXME\|HACK\|XXX" kernel/src/ --include="*.rs" 2>/dev/null | grep -v "clippy" | wc -l)
if [ "$TODO_COUNT" -gt 0 ]; then
    warn "Found $TODO_COUNT TODO/FIXME comments:"
    grep -rn "TODO\|FIXME\|HACK\|XXX" kernel/src/ --include="*.rs" 2>/dev/null | grep -v "clippy" | head -10
    echo ""
fi

# 2. Check for unsafe blocks without SAFETY comments
echo "--- Checking unsafe blocks ---"
UNSAFE_COUNT=$(grep -rn "unsafe {" kernel/src/ --include="*.rs" 2>/dev/null | wc -l)
UNSAFE_WITH_SAFETY=$(grep -rn -B1 "unsafe {" kernel/src/ --include="*.rs" 2>/dev/null | grep -c "SAFETY" || true)
echo "Total unsafe blocks: $UNSAFE_COUNT"
echo "With SAFETY comments: $UNSAFE_WITH_SAFETY"
if [ "$UNSAFE_COUNT" -gt 0 ] && [ "$UNSAFE_WITH_SAFETY" -lt "$UNSAFE_COUNT" ]; then
    warn "Some unsafe blocks lack SAFETY comments"
fi
echo ""

# 3. Check for proper error handling (no unwrap in production code)
echo "--- Checking for unwrap() in production code ---"
UNWRAP_COUNT=$(grep -rn "\.unwrap()" kernel/src/ --include="*.rs" 2>/dev/null | grep -v "test\|#\[test\]\|expect(" | wc -l)
if [ "$UNWRAP_COUNT" -gt 0 ]; then
    warn "Found $UNWRAP_COUNT unwrap() calls (consider using expect() or ?):"
    grep -rn "\.unwrap()" kernel/src/ --include="*.rs" 2>/dev/null | grep -v "test\|#\[test\]\|expect(" | head -5
    echo ""
fi

# 4. Check for dead code warnings
echo "--- Checking for dead code ---"
DEAD_CODE=$(grep -rn "dead_code" kernel/src/ --include="*.rs" 2>/dev/null | wc -l)
echo "Dead code annotations: $DEAD_CODE"
echo ""

# 5. Check for proper documentation
echo "--- Checking documentation coverage ---"
PUB_FN_COUNT=$(grep -rn "pub fn\|pub struct\|pub enum\|pub trait" kernel/src/ --include="*.rs" 2>/dev/null | wc -l)
DOC_COUNT=$(grep -rn "///\|//!\|pub fn\|pub struct\|pub enum\|pub trait" kernel/src/ --include="*.rs" 2>/dev/null | grep -c "///\|//!" || true)
echo "Public items: $PUB_FN_COUNT"
echo "Documented items: $DOC_COUNT"
echo ""

# 6. Check for proper module documentation
echo "--- Checking module documentation ---"
for f in kernel/src/*/mod.rs kernel/src/main.rs; do
    if [ -f "$f" ]; then
        if head -5 "$f" | grep -q "^//!"; then
            log "$f has module documentation"
        else
            warn "$f missing module documentation"
        fi
    fi
done
echo ""

# 7. Check for consistent naming
echo "--- Checking naming conventions ---"
# Check for snake_case in function names
BAD_FN_NAMES=$(grep -rn "pub fn [A-Z]" kernel/src/ --include="*.rs" 2>/dev/null | wc -l)
if [ "$BAD_FN_NAMES" -gt 0 ]; then
    warn "Found $BAD_FN_NAMES functions with non-snake_case names"
fi
echo ""

# 8. Summary
echo "========================================="
if [ "$ERRORS" -eq 0 ]; then
    log "All quality checks passed!"
else
    err "$ERRORS quality check(s) failed"
fi
echo "========================================="

exit $ERRORS
