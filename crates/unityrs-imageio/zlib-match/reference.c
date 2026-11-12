/* Test-only wrapper around pristine zlib 1.2.13's deflate.c (Zlib).
 * The included source is unchanged; only its exported names differ.
 * Shared trees, checksums and allocation helpers retain Z_PREFIX's names.
 */
#include "zconf.h"

#undef deflate
#undef deflateBound
#undef deflateCopy
#undef deflateEnd
#undef deflateGetDictionary
#undef deflateInit2_
#undef deflateInit_
#undef deflateParams
#undef deflatePending
#undef deflatePrime
#undef deflateReset
#undef deflateResetKeep
#undef deflateSetDictionary
#undef deflateSetHeader
#undef deflateTune
#undef deflate_copyright

#define deflate unityrs_zref_deflate
#define deflateBound unityrs_zref_deflateBound
#define deflateCopy unityrs_zref_deflateCopy
#define deflateEnd unityrs_zref_deflateEnd
#define deflateGetDictionary unityrs_zref_deflateGetDictionary
#define deflateInit2_ unityrs_zref_deflateInit2_
#define deflateInit_ unityrs_zref_deflateInit_
#define deflateParams unityrs_zref_deflateParams
#define deflatePending unityrs_zref_deflatePending
#define deflatePrime unityrs_zref_deflatePrime
#define deflateReset unityrs_zref_deflateReset
#define deflateResetKeep unityrs_zref_deflateResetKeep
#define deflateSetDictionary unityrs_zref_deflateSetDictionary
#define deflateSetHeader unityrs_zref_deflateSetHeader
#define deflateTune unityrs_zref_deflateTune
#define deflate_copyright unityrs_zref_deflate_copyright

#include "deflate.c"
