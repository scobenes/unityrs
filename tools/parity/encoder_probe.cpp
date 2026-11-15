#include <cstdint>
#include <cstddef>
#include <cstring>
#include "ProcessDxtc.hpp"
#include "ProcessRGB.hpp"
#include "bc7enc.h"
#include "astcenc.h"
extern "C" int etc(unsigned f,const unsigned char* in,unsigned w,unsigned h,unsigned char* out) {
 static const bool initialized=[](){bc7enc_compress_block_init();return true;}(); (void)initialized;
 bc7enc_compress_block_params p;bc7enc_compress_block_params_init(&p);
 for(unsigned y=0;y<h;y+=4)for(unsigned x=0;x<w;x+=4){alignas(64) uint32_t src[16];alignas(64) uint64_t dst[2];for(unsigned j=0;j<4;j++)memcpy(src+j*4,in+4*((y+j)*w+x),16);
 if(f>=34)for(auto& v:src)v=(v&0xFF00FF00)|((v&0x00FF0000)>>16)|((v&0xFF)<<16);
 switch(f){case 10:CompressBc1(src,dst,1,4);break;case 12:CompressBc3(src,dst,1,4);break;case 26:CompressBc4(src,dst,1,4);break;case 27:CompressBc5(src,dst,1,4);break;case 25:CompressBc7(src,dst,1,4,&p);break;case 34:CompressEtc1Rgb(src,dst,1,4);break;case 45:CompressEtc2Rgb(src,dst,1,4,true);break;case 47:CompressEtc2Rgba(src,dst,1,4,true);break;default:return 1;}
 unsigned n=f==10||f==26||f==34||f==45?8:16;memcpy(out,dst,n);out+=n;}return 0;
}
extern "C" int astc(const unsigned char* in,unsigned w,unsigned h,unsigned bx,unsigned by,unsigned char* out){astcenc_config cfg;auto e=astcenc_config_init(ASTCENC_PRF_LDR,bx,by,1,100,ASTCENC_FLG_USE_DECODE_UNORM8,&cfg);if(e)return e;astcenc_context* ctx=nullptr;e=astcenc_context_alloc(&cfg,1,&ctx);if(e)return e;void* slice=const_cast<unsigned char*>(in);astcenc_image image{w,h,1,ASTCENC_TYPE_U8,&slice};astcenc_swizzle swz{ASTCENC_SWZ_R,ASTCENC_SWZ_G,ASTCENC_SWZ_B,ASTCENC_SWZ_A};e=astcenc_compress_image(ctx,&image,&swz,out,((w+bx-1)/bx)*((h+by-1)/by)*16,0);astcenc_context_free(ctx);return e;}
#include <fstream>
#include <vector>
#include <cstdint>
extern "C" int etc(unsigned,const unsigned char*,unsigned,unsigned,unsigned char*);
extern "C" int astc(const unsigned char*,unsigned,unsigned,unsigned,unsigned,unsigned char*);
int main(int argc,char** argv){if(argc!=3)return 1;std::ifstream input(argv[1],std::ios::binary);std::ofstream output(argv[2],std::ios::binary);uint32_t h[7];while(input.read((char*)h,sizeof(h))){std::vector<unsigned char> in(h[5]),out(h[6]);input.read((char*)in.data(),in.size());int r=h[0]?etc(h[0],in.data(),h[1],h[2],out.data()):astc(in.data(),h[1],h[2],h[3],h[4],out.data());if(r)return r;output.write((char*)out.data(),out.size());}return 0;}
