# 📚 Remote-Retro Architecture & Research Documentation

This directory contains the mathematical models, empirical benchmark reports, and theoretical research tracks for **Remote-Retro**:

---

## Document Index

1. **[GBA Cloud Streaming: Proven Architecture & Real WAN Benchmarks (`docs/gba.md`)](gba.md)**
   - **Status**: ✅ **PROVEN & IMPLEMENTED**
   - Covers 2D PPU state-delta streaming, 180 B/frame video payload, run-ahead lag reduction, RetroArch calibration dock, dedicated Web Worker architecture, and real WAN benchmark results over Tailscale.

2. **[3D Console & PSP Streaming: Information Theory & Hypotheses (`docs/psp.md`)](psp.md)**
   - **Status**: 🧪 **HYPOTHESIS / RESEARCH**
   - Covers negative proofs from PPSSPP source code (why command streaming expands bandwidth by 40× and halts on blocking readbacks), and 4 information-theoretic hybrid tracks (Depth Reprojection, Symbolic Audio, WebGPU FSR, Motion Vector Extrapolation).

---

## Mathematical Foundations
* **2D Tile Consoles (GBA, SNES, NES)**: $\text{Entropy}(\Delta \text{State}) \ll \text{Entropy}(\text{Video})$. State-delta streaming yields a $20\times$ bandwidth win and bit-exact lossless pixels.
* **3D Geometry Consoles (PSP, PS1, GameCube, Wii)**: $\text{Entropy}(\text{Video}) \ll \text{Entropy}(\text{Vertices})$. Information-aware hybrid video streaming (depth warping, symbolic audio, client super-resolution) yields the optimal latency/bandwidth Pareto frontier.
