#!/bin/bash
set -e

MGBA_DIR="/home/cyc/mgba"
OUTPUT_WASM="/home/cyc/gba_streamer/static/gba_ppu.wasm"

echo "=== Building Standalone mGBA WASM PPU ==="
clang -O3 --target=wasm32-wasi --sysroot=/usr/wasm32-wasi \
  -I"$MGBA_DIR/include" -I"$MGBA_DIR/src" \
  -c "$MGBA_DIR/src/gba/renderers/video-software.c" -o /tmp/video-software.o

clang -O3 --target=wasm32-wasi --sysroot=/usr/wasm32-wasi \
  -I"$MGBA_DIR/include" -I"$MGBA_DIR/src" \
  -c "$MGBA_DIR/src/gba/renderers/software-obj.c" -o /tmp/software-obj.o

clang -O3 --target=wasm32-wasi --sysroot=/usr/wasm32-wasi \
  -I"$MGBA_DIR/include" -I"$MGBA_DIR/src" \
  -c "$MGBA_DIR/src/gba/renderers/software-mode0.c" -o /tmp/software-mode0.o

clang -O3 --target=wasm32-wasi --sysroot=/usr/wasm32-wasi \
  -I"$MGBA_DIR/include" -I"$MGBA_DIR/src" \
  -c "$MGBA_DIR/src/gba/renderers/software-bg.c" -o /tmp/software-bg.o

clang -O3 --target=wasm32-wasi --sysroot=/usr/wasm32-wasi \
  -I"$MGBA_DIR/include" -I"$MGBA_DIR/src" \
  -c "$MGBA_DIR/src/gba/renderers/common.c" -o /tmp/common.o

clang -O3 --target=wasm32-wasi --sysroot=/usr/wasm32-wasi \
  -I"$MGBA_DIR/include" -I"$MGBA_DIR/src" \
  -c "$MGBA_DIR/src/gba/renderers/wasm_ppu_bridge.c" -o /tmp/wasm_ppu_bridge.o

wasm-ld -m wasm32 --no-entry --allow-undefined \
  --initial-memory=4194304 --max-memory=16777216 \
  /tmp/wasm_ppu_bridge.o /tmp/video-software.o /tmp/software-obj.o \
  /tmp/software-mode0.o /tmp/software-bg.o /tmp/common.o \
  /usr/wasm32-wasi/lib/wasm32-wasi/libc.a \
  -o "$OUTPUT_WASM"

chmod 755 "$OUTPUT_WASM"
echo "✅ Built $OUTPUT_WASM ($(ls -lh "$OUTPUT_WASM" | awk '{print $5}'))"
