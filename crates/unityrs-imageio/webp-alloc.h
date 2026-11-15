// Allocator entry points supplied by src/webp_ffi.rs. Applied to every C unit.
#include <stddef.h>
void* unityrs_webp_malloc(size_t size);
void* unityrs_webp_calloc(size_t count, size_t size);
void unityrs_webp_free(void* ptr);
#define malloc unityrs_webp_malloc
#define calloc unityrs_webp_calloc
#define free unityrs_webp_free
