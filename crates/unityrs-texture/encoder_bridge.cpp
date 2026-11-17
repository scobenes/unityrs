// Narrow encoder boundary. Source parameters match the pinned oracle; the
// independent wrapper validates every size before an upstream function sees it.
#include <cstddef>
#include <cstdint>
#include <cstring>
#include <limits>
#include <new>
#include "ProcessDxtc.hpp"
#include "ProcessRGB.hpp"
#include "bc7enc.h"
#include "astcenc.h"
#include "encoder_allocator.hpp"
using Alloc = void* (*)(void*, std::size_t, std::size_t);
using Free = void (*)(void*, void*, std::size_t, std::size_t);
namespace {
struct Allocation {void* pointer; std::size_t size; std::size_t alignment; void (*destroy)(void*) = nullptr;};
struct Allocator {
    void* user; Alloc alloc; Free free;
    Allocation live[64]{};
    ~Allocator() { for (auto& a:live) if(a.pointer) {if(a.destroy) a.destroy(a.pointer);free(user,a.pointer,a.size,a.alignment);} }
};
thread_local Allocator* active = nullptr;
struct Active {
    explicit Active(Allocator& value) {active=&value;}
    ~Active() {active=nullptr;}
};
bool input_size(const std::uint8_t* rgba,std::size_t len,std::uint32_t w,std::uint32_t h) {
    return rgba && w && h && std::uint64_t(w)*h <= std::numeric_limits<std::size_t>::max()/4 && len==std::size_t(w)*h*4;
}
}
void* unityrs_astc_allocate(std::size_t size,std::size_t alignment) {
    if(!active) throw std::bad_alloc();
    if(alignment<alignof(void*)) alignment=alignof(void*);
    for(auto& a:active->live) if(!a.pointer) {
        void* p=active->alloc(active->user,size,alignment);
        if(!p) throw std::bad_alloc();
        a={p,size,alignment}; return p;
    }
    throw std::bad_alloc();
}
void unityrs_astc_destructor(void* p, void (*destroy)(void*)) noexcept {
    for(auto& a:active->live) if(a.pointer==p){a.destroy=destroy;return;}
}
void unityrs_astc_release(const void* p) noexcept {
    if(!p || !active)return;
    for(auto& a:active->live) if(a.pointer==p){if(a.destroy) a.destroy(a.pointer);active->free(active->user,a.pointer,a.size,a.alignment);a={};return;}
}
extern "C" int unityrs_etcpak_encode(std::uint32_t f,const std::uint8_t* rgba,std::size_t len,std::uint32_t w,std::uint32_t h,std::uint8_t* out,std::size_t cap) noexcept {
    if(!input_size(rgba,len,w,h)||w%4||h%4||!out)return 1;
    unsigned n;
    switch(f){case 10:case 26:case 34:case 60:case 45:n=8;break;case 12:case 27:case 25:case 47:n=16;break;default:return 1;}
    if(std::uint64_t(w/4)*(h/4)*n!=cap)return 1;
    try {
        static const bool initialized=[](){bc7enc_compress_block_init();return true;}();(void)initialized;
        bc7enc_compress_block_params p{};bc7enc_compress_block_params_init(&p);
        for(std::uint32_t y=0;y<h;y+=4)for(std::uint32_t x=0;x<w;x+=4){
            alignas(64) std::uint32_t src[16];alignas(64) std::uint64_t dst[2]{};
            for(unsigned j=0;j<4;j++)std::memcpy(src+j*4,rgba+4*((std::size_t(y)+j)*w+x),16);
            if(f>=34)for(auto& v:src)v=(v&0xFF00FF00)|((v&0x00FF0000)>>16)|((v&0xFF)<<16);
            switch(f){
            case 10:CompressBc1(src,dst,1,4);break;
            case 12:CompressBc3(src,dst,1,4);break;
            case 26:CompressBc4(src,dst,1,4);break;
            case 27:CompressBc5(src,dst,1,4);break;
            case 25:CompressBc7(src,dst,1,4,&p);break;
            case 34:case 60:CompressEtc1Rgb(src,dst,1,4);break;
            case 45:CompressEtc2Rgb(src,dst,1,4,true);break;
            case 47:CompressEtc2Rgba(src,dst,1,4,true);break;
            }
            std::memcpy(out,dst,n);out+=n;
        }
        return 0;
    }catch(const std::bad_alloc&){return 2;}catch(...){return 4;}
}
extern "C" int unityrs_astc_encode(const std::uint8_t* rgba,std::size_t len,std::uint32_t w,std::uint32_t h,std::uint32_t bx,std::uint32_t by,std::uint8_t* out,std::size_t cap,void* user,Alloc alloc,Free free) noexcept {
    if(!input_size(rgba,len,w,h)||!out||!alloc||!free||active)return 1;
    if(!bx||!by||bx>12||by>12)return 1;
    const auto blocks=((std::uint64_t(w)+bx-1)/bx)*((std::uint64_t(h)+by-1)/by);
    if(blocks>std::numeric_limits<std::size_t>::max()/16||blocks*16!=cap)return 1;
    Allocator state{user,alloc,free};Active scope(state);
    try {
        astcenc_config config;
        auto error=astcenc_config_init(ASTCENC_PRF_LDR,bx,by,1,100,ASTCENC_FLG_USE_DECODE_UNORM8,&config);
        if(error!=ASTCENC_SUCCESS)return 3;
        astcenc_context* ctx=nullptr;
        error=astcenc_context_alloc(&config,1,&ctx);
        if(error!=ASTCENC_SUCCESS)return error==ASTCENC_ERR_OUT_OF_MEM?2:3;
        void* slice=const_cast<std::uint8_t*>(rgba);
        astcenc_image image{w,h,1,ASTCENC_TYPE_U8,&slice};
        astcenc_swizzle swizzle{ASTCENC_SWZ_R,ASTCENC_SWZ_G,ASTCENC_SWZ_B,ASTCENC_SWZ_A};
        error=astcenc_compress_image(ctx,&image,&swizzle,out,cap,0);
        astcenc_context_free(ctx);
        return error==ASTCENC_SUCCESS?0:error==ASTCENC_ERR_OUT_OF_MEM?2:3;
    }catch(const std::bad_alloc&){return 2;}catch(...){return 4;}
}
