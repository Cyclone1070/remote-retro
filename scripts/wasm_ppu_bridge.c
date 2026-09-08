#include <stdint.h>
#include <stddef.h>
#include <string.h>

#include "gba/renderers/software-private.h"
#include <mgba/internal/gba/video.h>
#include <mgba/internal/gba/io.h>
#include <mgba/core/log.h>

const int GBAVideoObjSizes[16][2] = {
	{ 8, 8 },
	{ 16, 16 },
	{ 32, 32 },
	{ 64, 64 },
	{ 16, 8 },
	{ 32, 8 },
	{ 32, 16 },
	{ 64, 32 },
	{ 8, 16 },
	{ 8, 32 },
	{ 16, 32 },
	{ 32, 64 },
	{ 0, 0 },
	{ 0, 0 },
	{ 0, 0 },
	{ 0, 0 }
};

int _mLOG_CAT_GBA_VIDEO = 0;
void mLog(int category, enum mLogLevel level, const char* format, ...) {}
void GBAVideoCacheWriteVideoRegister(void* cache, uint32_t address, uint16_t value) {}
void mCacheSetWriteVRAM(void* cache, uint32_t address) {}
void mCacheSetWritePalette(void* cache, uint32_t address, uint16_t value) {}
void mappedMemoryFree(void* p, size_t size) {}

static uint8_t s_vram[98304] __attribute__((aligned(4)));
static uint16_t s_palette[512] __attribute__((aligned(4)));
static union GBAOAM s_oam __attribute__((aligned(4)));
static uint16_t s_io[64] __attribute__((aligned(4)));
static uint32_t s_outputBuffer[240 * 160] __attribute__((aligned(4)));

static struct GBAVideoSoftwareRenderer s_renderer;

__attribute__((export_name("get_vram_ptr")))
uint8_t* get_vram_ptr(void) {
    return s_vram;
}

__attribute__((export_name("get_palette_ptr")))
uint16_t* get_palette_ptr(void) {
    return s_palette;
}

__attribute__((export_name("get_oam_ptr")))
uint8_t* get_oam_ptr(void) {
    return (uint8_t*)&s_oam;
}

__attribute__((export_name("get_io_ptr")))
uint16_t* get_io_ptr(void) {
    return s_io;
}

__attribute__((export_name("get_output_ptr")))
uint32_t* get_output_ptr(void) {
    return s_outputBuffer;
}

__attribute__((export_name("ppu_init")))
void ppu_init(void) {
    GBAVideoSoftwareRendererCreate(&s_renderer);
    s_renderer.outputBuffer = s_outputBuffer;
    s_renderer.outputBufferStride = 240;
    s_renderer.d.vram = (uint16_t*)s_vram;
    s_renderer.d.palette = s_palette;
    s_renderer.d.oam = &s_oam;
    s_renderer.d.init(&s_renderer.d);
}

__attribute__((export_name("ppu_render_frame")))
void ppu_render_frame(void) {
    // 1. Sync video IO registers (offsets 0x000 to 0x056)
    for (uint32_t i = 0; i < 44; ++i) {
        uint32_t reg_addr = i * 2;
        s_renderer.d.writeVideoRegister(&s_renderer.d, reg_addr, s_io[i]);
    }

    // 2. Sync palette to normalPalette
    static uint16_t s_prev_palette[512];
    static int s_palette_initialized = 0;
    if (!s_palette_initialized) {
        for (uint32_t i = 0; i < 512; ++i) {
            s_renderer.d.writePalette(&s_renderer.d, i * 2, s_palette[i]);
            s_prev_palette[i] = s_palette[i];
        }
        s_palette_initialized = 1;
    } else {
        for (uint32_t i = 0; i < 512; ++i) {
            if (s_palette[i] != s_prev_palette[i]) {
                s_renderer.d.writePalette(&s_renderer.d, i * 2, s_palette[i]);
                s_prev_palette[i] = s_palette[i];
            }
        }
    }

    // 3. Mark OAM and VRAM scanlines dirty
    s_renderer.oamDirty = true;
    memset(s_renderer.scanlineDirty, 0xFFFFFFFF, sizeof(s_renderer.scanlineDirty));
    s_renderer.bg[0].yCache = -1;
    s_renderer.bg[1].yCache = -1;
    s_renderer.bg[2].yCache = -1;
    s_renderer.bg[3].yCache = -1;

    // 3. Draw all 160 scanlines
    for (int y = 0; y < 160; ++y) {
        s_renderer.d.drawScanline(&s_renderer.d, y);
    }

    // 4. Finish frame
    s_renderer.d.finishFrame(&s_renderer.d);
}
