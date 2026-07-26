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
if [ "$UNSAFE_COUNT" -gt 0 ]; then
    SAFETY_RATIO=$((UNSAFE_WITH_SAFETY * 100 / UNSAFE_COUNT))
    echo "SAFETY coverage: ${SAFETY_RATIO}%"
    if [ "$SAFETY_RATIO" -lt 30 ]; then
        err "SAFETY comment coverage below 30% (${SAFETY_RATIO}%)"
    elif [ "$SAFETY_RATIO" -lt 60 ]; then
        warn "Some unsafe blocks lack SAFETY comments (${SAFETY_RATIO}% coverage)"
    else
        log "SAFETY comment coverage: ${SAFETY_RATIO}%"
    fi
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

# 5. Check for proper documentation (doc comments on public items)
echo "--- Checking documentation coverage ---"
PUB_FN_COUNT=$(grep -rn "pub fn\|pub struct\|pub enum\|pub trait" kernel/src/ --include="*.rs" 2>/dev/null | grep -v "#\[test\]" | grep -v "// pub\|//.*pub fn\|//.*pub struct" | wc -l)
DOC_COUNT=$(grep -rn -B1 "pub fn\|pub struct\|pub enum\|pub trait" kernel/src/ --include="*.rs" 2>/dev/null | grep -c "///\|//!" || true)
echo "Public items: $PUB_FN_COUNT"
echo "Items with doc comments: $DOC_COUNT"
if [ "$PUB_FN_COUNT" -gt 0 ]; then
    DOC_RATIO=$((DOC_COUNT * 100 / PUB_FN_COUNT))
    echo "Documentation coverage: ${DOC_RATIO}%"
    if [ "$DOC_RATIO" -lt 50 ]; then
        warn "Documentation coverage below 50% (${DOC_RATIO}%)"
    else
        log "Documentation coverage: ${DOC_RATIO}%"
    fi
fi
echo ""

# 5b. Check for doc comments specifically on public functions (more precise)
echo "--- Checking doc comments on public functions ---"
PUB_FNS=$(grep -rn "pub fn " kernel/src/ --include="*.rs" 2>/dev/null | grep -v "#\[test\]" | grep -v "// pub fn" | wc -l)
PUB_FNS_WITH_DOCS=$(grep -rn -B1 "pub fn " kernel/src/ --include="*.rs" 2>/dev/null | grep -v "#\[test\]" | grep -A0 "pub fn " | grep -B1 "pub fn " | grep -c "///" || true)
echo "Public functions: $PUB_FNS"
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

# 8. Check SAFETY comments on all unsafe blocks
echo "--- Checking SAFETY comments on unsafe blocks ---"
UNSAFE_EXPR_COUNT=$(grep -rn "unsafe " kernel/src/ --include="*.rs" 2>/dev/null | grep -v "unsafe fn\|unsafe impl\|unsafe trait\|#\[allow\|clippy" | wc -l)
SAFETY_COMMENT_COUNT=$(grep -rn "SAFETY" kernel/src/ --include="*.rs" 2>/dev/null | wc -l)
echo "Unsafe expressions: $UNSAFE_EXPR_COUNT"
echo "SAFETY comments: $SAFETY_COMMENT_COUNT"
if [ "$UNSAFE_EXPR_COUNT" -gt 0 ]; then
    SAFETY_PCT=$((SAFETY_COMMENT_COUNT * 100 / UNSAFE_EXPR_COUNT))
    echo "SAFETY comment density: ${SAFETY_PCT}%"
fi
echo ""

# 9. Check test coverage (count #[test] functions)
echo "--- Checking test coverage ---"
TEST_COUNT=$(grep -rn "#\[test\]" kernel/src/ --include="*.rs" 2>/dev/null | wc -l)
echo "Total #[test] functions: $TEST_COUNT"
if [ "$TEST_COUNT" -lt 100 ]; then
    warn "Low test count: $TEST_COUNT (consider adding more tests)"
else
    log "Test count: $TEST_COUNT"
fi

# Count test modules per file
echo "Test distribution:"
for f in $(find kernel/src -name "*.rs" | sort); do
    count=$(grep -c "#\[test\]" "$f" 2>/dev/null || true)
    if [ "$count" -gt 0 ]; then
        echo "  $f: $count tests"
    fi
done
echo ""

# 10. Summary
echo "========================================="
if [ "$ERRORS" -eq 0 ]; then
    log "All quality checks passed!"
else
    err "$ERRORS quality check(s) failed"
fi
echo "========================================="

exit $ERRORS
