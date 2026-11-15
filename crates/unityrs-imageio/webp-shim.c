// Ported from Pillow 9.5.0 (MIT-CMU): src/_webp.c's WebPEncode_wrapper, still images without metadata. See THIRD_PARTY_LICENSES.
// Config/import semantics: src/_webp.c:586-624. The bounded writer replaces
// WebPMemoryWriter; with no metadata its RIFF is already the mux's output.
#include <string.h>
#include "src/webp/encode.h"
#include "src/dsp/dsp.h"
#include "src/dsp/lossless.h"

// Ported from libwebp 1.3.0 (BSD-3-Clause): picture_csp_enc.c's Import, using byte reads for arbitrary Rust slice alignment. See THIRD_PARTY_LICENSES.
static int ImportRGBA(WebPPicture* pic, const uint8_t* rgba) {
  int x, y;
  if (!WebPPictureAlloc(pic)) return 0;
  VP8LDspInit();
  WebPInitAlphaProcessing();
  for (y = 0; y < pic->height; ++y) {
    uint32_t* dst = pic->argb + y * pic->argb_stride;
    for (x = 0; x < pic->width; ++x) {
      dst[x] = ((uint32_t)rgba[3] << 24) | ((uint32_t)rgba[0] << 16) |
               ((uint32_t)rgba[1] << 8) | (uint32_t)rgba[2];
      rgba += 4;
    }
  }
  return 1;
}

typedef struct {
  uint8_t* bytes;
  size_t capacity;
  size_t size;
} UnityrsWriter;

static int Write(const uint8_t* data, size_t size, const WebPPicture* pic) {
  UnityrsWriter* w = (UnityrsWriter*)pic->custom_ptr;
  if (size > w->capacity - w->size) return 0;
  memcpy(w->bytes + w->size, data, size);
  w->size += size;
  return 1;
}

int unityrs_webp_encode(const uint8_t* rgb, int width, int height, int alpha,
                        float quality, int method, int exact, int lossless,
                        uint8_t* output, size_t capacity, size_t* size) {
  WebPConfig config;
  WebPPicture pic;
  UnityrsWriter writer = {output, capacity, 0};
  int ok, error;
  if (!WebPConfigInit(&config) || !WebPPictureInit(&pic)) return -1;
  config.lossless = lossless;
  config.quality = quality;
  config.method = method;
  config.exact = exact;
  if (!WebPValidateConfig(&config)) return VP8_ENC_ERROR_INVALID_CONFIGURATION;
  pic.width = width;
  pic.height = height;
  pic.use_argb = 1;
  ok = alpha ? ImportRGBA(&pic, rgb)
             : WebPPictureImportRGB(&pic, rgb, 3 * width);
  if (ok) {
    pic.writer = Write;
    pic.custom_ptr = &writer;
    ok = WebPEncode(&config, &pic);
  }
  // A false encoder result must never become success if error_code stayed OK.
  error = ok ? 0 : (pic.error_code != VP8_ENC_OK ? pic.error_code : -2);
  WebPPictureFree(&pic);
  *size = writer.size;
  return error;
}
