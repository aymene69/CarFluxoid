#define main phone_program_main
#include "../src/aa_phone.c"
#undef main
#include <assert.h>
static void check_poc_bit_copy(void){
 unsigned char source[2048];
 for(size_t i=0;i<sizeof source;i++)source[i]=(unsigned char)(i*37+i/7);
 struct poc_bits bits={source,sizeof source*8,0,0};
 for(size_t from=0;from<8;from++)for(size_t prefix=0;prefix<8;prefix++)
 for(size_t tail=0;tail<8;tail++){
  unsigned char actual[2049]={0},expected[2049]={0};
  struct poc_out fast={actual,sizeof actual,0,0},slow={expected,sizeof expected,0,0};
  for(size_t i=0;i<prefix;i++){poc_put(&fast,1);poc_put(&slow,1);}
  size_t end=sizeof source*8-tail;
  poc_copy(&fast,&bits,from,end);
  for(size_t i=from;i<end;i++)poc_put(&slow,source[i/8]>>(7-(i%8))&1);
  assert(!fast.err&&fast.bits==slow.bits&&!memcmp(actual,expected,sizeof actual));
 }
}
int main(void){
 check_poc_bit_copy();
 char path[100];snprintf(path,sizeof path,"/tmp/aa-video-options-%ld",(long)getpid());setenv("AA_SETTINGS_FILE",path,1);
 unsigned char offer[]={10,18,8,2,26,14,8,3,34,2,8,3,34,2,8,1,34,2,8,2};
 FILE *f=fopen(path,"w");assert(f);fputs("max_resolution=480\n",f);fclose(f);
 assert(find_video(offer,sizeof offer)==2&&selected_video_config==1&&display_width==800&&display_height==480);
 f=fopen(path,"w");fputs("max_resolution=720\n",f);fclose(f);
 assert(find_video(offer,sizeof offer)==2&&selected_video_config==2&&display_width==1280&&display_height==720);
 f=fopen(path,"w");fputs("max_resolution=0\n",f);fclose(f);
 assert(find_video(offer,sizeof offer)==2&&selected_video_config==0&&display_width==1920&&display_height==1080);
 unsigned char only720[]={10,10,8,2,26,6,8,3,34,2,8,2};
 f=fopen(path,"w");fputs("max_resolution=480\n",f);fclose(f);
 assert(find_video(only720,sizeof only720)==2&&selected_video_config==0&&display_height==720);
 unsigned char unknown[]={10,10,8,2,26,6,8,3,34,2,8,99};assert(find_video(unknown,sizeof unknown)==-1);
 unsigned char hevc_first[]={10,10,8,3,26,6,8,7,34,2,8,2,10,10,8,6,26,6,8,3,34,2,8,1};
 setenv("AA_VIDEO_SOCKET","test",1);
 assert(find_video(hevc_first,sizeof hevc_first)==6&&selected_codec==3);
 assert(find_video(hevc_first,12)==-1); /* Never reinterpret HEVC as H264. */
 unsetenv("AA_VIDEO_SOCKET");
 assert(find_video(hevc_first,sizeof hevc_first)==3&&selected_codec==7);
 unlink(path);
 unsigned char unit[]={0,0,0,1,0x67,0x42,0xc0,0x1e,0,0,0,1,0x68,0xce,0x3c,0x80,0,0,1,6,5,1,0x80,0,0,0,1,0x65,0x88,0x84},out[sizeof unit];size_t config_size;
 assert(split_parameter_sets(unit,sizeof unit,out,sizeof out,&config_size)==7&&config_size==16);
 assert(!memcmp(config_scratch,unit,16)&&!memcmp(out,unit+23,7)); /* SEI dropped, IDR kept */
 unsigned char two[]={0,0,0,1,0x67,0x42,0,0,0,1,0x68,0xce,0,0,0,1,0x65,0x88,0,0,0,1,0x67,0x42,0,0,0,1,0x68,0xce,0,0,0,1,0x65,0xb8};
 waiting_video=two;waiting_size=sizeof two;index_waiting_units();
 assert(waiting_units==2&&waiting_unit_off[0]==0&&waiting_unit_len[0]==18&&waiting_unit_off[1]==18&&waiting_unit_len[1]==18);
 assert(initial_projection_screen(1)==PROJECTION_MENU);
 assert(initial_projection_screen(0)==PROJECTION_WAITING);
 /* The waiting image stays ineligible between menu redraws, even long after startup. */
 assert(!waiting_picture_due(PROJECTION_MENU,2000000,0));
 assert(waiting_picture_due(PROJECTION_WAITING,2000000,0));
 assert(!waiting_picture_due(PROJECTION_WAITING,2000000,3000000));
 assert(!waiting_picture_due(PROJECTION_CARPLAY,2000000,0));
 assert(projection_after_disconnect(PROJECTION_MENU,1)==PROJECTION_MENU);
 assert(projection_after_disconnect(PROJECTION_WAITING,1)==PROJECTION_WAITING);
 assert(projection_after_disconnect(PROJECTION_CARPLAY,1)==PROJECTION_MENU);
 assert(projection_after_disconnect(PROJECTION_CARPLAY,0)==PROJECTION_WAITING);
 waiting_video=NULL;waiting_size=0;
 unsigned char slice[]={0,0,0,1,0x41,0x9a,0x10};
 assert(split_parameter_sets(slice,sizeof slice,out,sizeof out,&config_size)==sizeof slice&&!config_size&&!memcmp(out,slice,sizeof slice));
 assert(has_idr(unit,sizeof unit)&&!has_idr(slice,sizeof slice)); /* CarPlay resumes only on an IDR */
 /* Real iPhone CarPlay SPS (POC type 0, VUI max_num_reorder_frames 0): rewritten to POC type 2,
  * byte result checked against ffmpeg's parser offline. A SPS without VUI reorder info is kept. */
 unsigned char iphone_sps[]={0x27,0x64,0x00,0x1f,0xac,0x13,0x14,0x50,0x32,0x0f,0x69,0xb8,0x08,0x68,0x30,0x36,0x82,0x21,0x19,0x60};
 unsigned char poc2_sps[]={0x27,0x64,0x00,0x1f,0xac,0x12,0xca,0x06,0x41,0xed,0x37,0x01,0x0d,0x06,0x06,0xd0,0x44,0x23,0x2c};
 unsigned char rewritten[64];
 assert(poc_rewrite_sps(iphone_sps,sizeof iphone_sps,rewritten,sizeof rewritten,0)==sizeof poc2_sps&&!memcmp(rewritten,poc2_sps,sizeof poc2_sps)&&poc_state.active);
 unsigned char no_vui[]={0x27,0x64,0x00,0x1f,0xac,0x13,0x14,0x50,0x32,0x0f,0x64};
 assert(!poc_rewrite_sps(no_vui,sizeof no_vui,rewritten,sizeof rewritten,0)&&!poc_state.active);
 puts("Resolution cap, nonzero configuration index, automatic mode, receiver fallback, SPS/PPS codec config split, waiting IDR alternation and POC type 2 rewrite: PASS");
}
