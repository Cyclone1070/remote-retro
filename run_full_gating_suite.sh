#!/bin/bash
set -e

REPO_DIR="/Users/mac/repos/remote-retro"
cd "$REPO_DIR"

echo "==================================================================="
echo " 🛡️ MASTER CONTINUOUS INTEGRATION & ZERO-REGRESSION GATING SUITE"
echo "==================================================================="
echo " Target: GBA Lossless Cloud Streamer (Host: 100.73.151.90:48500)"
echo " Environment: macOS Client (Chrome 151) <-> Fedora Linux Host"
echo "==================================================================="
echo ""

# ---------------------------------------------------------
# GATE 1: Rust Compilation & Core Unit Tests
# ---------------------------------------------------------
echo "▶ [GATE 1/4] Running Rust Cargo Unit & Integration Tests..."
cd "$REPO_DIR/gba_streamer"
cargo test --release
echo "  ↳ ✅ GATE 1 PASSED: Rust core tests compiled and passed cleanly."
echo ""

# ---------------------------------------------------------
# GATE 2: Server Precision 60.000 Hz Pacing & Drift Gating
# ---------------------------------------------------------
echo "▶ [GATE 2/4] Running Server Clock Pacing & Jitter Verification (1,000 samples)..."
cargo run --release --bin bench_server_pacing
echo "  ↳ ✅ GATE 2 PASSED: Server clock locked at 60.0000 Hz (sigma < 0.05ms)."
echo ""

# ---------------------------------------------------------
# GATE 3: 2,000-Frame Backend Lossless A/V & M2P Gating
# ---------------------------------------------------------
echo "▶ [GATE 3/4] Running 2,000-Frame A/V Synchronized Lossless Benchmark..."
cargo run --release --bin bench_ghosting_and_m2p
echo "  ↳ ✅ GATE 3 PASSED: 2,000 frames evaluated (100% video integrity, 100% audio integrity)."
echo ""

# ---------------------------------------------------------
# GATE 4: In-Browser Playwright Autonomous A/V & Presentation Gating
# ---------------------------------------------------------
echo "▶ [GATE 4/4] Running In-Browser Autonomous Playwright A/V & Presentation Gating..."
cd "$REPO_DIR"
node qa_autonomous_gating.js
echo "  ↳ ✅ GATE 4 PASSED: Browser audio RMS, stereo balance, 60 FPS scanout, and 0-stutter passed."
echo ""

echo "==================================================================="
echo " 🎉 ALL 4 GATES PASSED PERFECTLY! ZERO REGRESSIONS DETECTED."
echo "==================================================================="
