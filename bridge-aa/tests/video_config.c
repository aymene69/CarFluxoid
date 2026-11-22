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
static void check_poc_slice_fast_equivalence(void){
 unsigned char iphone_sps[]={0x27,0x64,0x00,0x1f,0xac,0x13,0x14,0x50,0x32,0x0f,0x69,0xb8,0x08,0x68,0x30,0x36,0x82,0x21,0x19,0x60};
 unsigned char iphone_pps[]={0x28,0xee,0x3c,0xb0};
 unsigned char sps_out[64];
 assert(poc_rewrite_sps(iphone_sps,sizeof iphone_sps,sps_out,sizeof sps_out,0)>0&&poc_state.active);
 poc_parse_pps(iphone_pps,sizeof iphone_pps);
 assert(poc_state.pps_ok&&poc_state.cabac);
 static const unsigned char payloads[][8]={
  {0x00,0x00,0x00,0x01,0xaa,0xbb,0xcc,0xdd},
  {0x00,0x00,0x02,0x03,0x00,0x00,0x01,0x80},
  {0x01,0x02,0x03,0x00,0x00,0x00,0x00,0x80},
  {0x02,0x00,0x00,0x03,0xaa,0x00,0x00,0x80},
  {0x03,0x00,0x00,0x00,0x03,0x00,0x01,0x80},
  {0x00,0x01,0x00,0x00,0x02,0x00,0x00,0x80},
  {0x00,0x02,0x00,0x00,0x03,0x00,0x00,0x80},
  {0xff,0x00,0x00,0x01,0x00,0x00,0x02,0x80},
 };
 /* Build valid CABAC IDR (nal=0x65) and P (nal=0x41) slices with varying header bit patterns
  * (including trailing 0x00 bytes before CABAC alignment) and compare fast vs full byte-for-byte. */
 for(int is_idr=0;is_idr<=1;is_idr++)
 for(unsigned fnum=0;fnum<4;fnum++)
 for(unsigned poc_lsb=0;poc_lsb<4;poc_lsb++)
 for(unsigned p_idx=0;p_idx<sizeof payloads/sizeof *payloads;p_idx++){
  unsigned char rbsp[128]={0};
  struct poc_out hdr={rbsp,sizeof rbsp,0,0};
  /* first_mb_in_slice = 0 -> ue(0) = 1 */
  poc_put(&hdr,1);
  /* slice_type: IDR=2 (I -> ue(2)=011), P=0 (ue(0)=1) */
  if(is_idr){poc_put(&hdr,0);poc_put(&hdr,1);poc_put(&hdr,1);}
  else poc_put(&hdr,1);
  /* pic_parameter_set_id = 0 -> ue(0) = 1 */
  poc_put(&hdr,1);
  /* frame_num (log2_frame_num bits) */
  for(unsigned k=poc_state.log2_frame_num;k--;)poc_put(&hdr,(fnum>>k)&1);
  /* idr_pic_id = 0 -> ue(0) = 1 */
  if(is_idr)poc_put(&hdr,1);
  /* pic_order_cnt_lsb (log2_poc_lsb bits) */
  for(unsigned k=poc_state.log2_poc_lsb;k--;)poc_put(&hdr,(poc_lsb>>k)&1);
  if(!is_idr){
   poc_put(&hdr,0); /* num_ref_idx_active_override_flag = 0 */
   poc_put(&hdr,0); /* ref_pic_list_modification_flag_l0 = 0 */
   poc_put(&hdr,0); /* adaptive_ref_pic_marking_mode_flag = 0 */
   poc_put(&hdr,1); /* cabac_init_idc = 0 -> ue(0) = 1 */
  }else{
   poc_put(&hdr,0);poc_put(&hdr,0); /* no_output_of_prior_pics, long_term_reference */
  }
  /* slice_qp_delta = 0 -> se(0) = 1 */
  poc_put(&hdr,1);
  if(poc_state.deblocking){
   /* disable_deblocking_filter_idc = 0 (ue(0)=1), alpha=0 (1), beta=0 (1) */
   poc_put(&hdr,1);poc_put(&hdr,1);poc_put(&hdr,1);
  }
  while(hdr.bits&7)poc_put(&hdr,1); /* cabac_alignment_one_bit */
  size_t hbytes=hdr.bits/8;
  /* Append payload pattern (and for one case, repeat to 64 KiB). */
  size_t plen=sizeof payloads[p_idx];
  size_t rbsp_len=hbytes+plen;
  memcpy(rbsp+hbytes,payloads[p_idx],plen);
  unsigned char nal[256],fast_out[256],full_out[256];
  nal[0]=is_idr?0x65:0x41;
  size_t esc_len=poc_escape(rbsp,rbsp_len,nal+1,sizeof nal-1);
  assert(esc_len>0);
  size_t nal_len=esc_len+1;
  size_t flen=poc_rewrite_slice(nal,nal_len,fast_out,sizeof fast_out);
  size_t rlen=poc_rewrite_slice_full(nal,nal_len,full_out,sizeof full_out);
  assert(flen>0&&flen==rlen&&!memcmp(fast_out,full_out,flen));
 }
 /* Large 64 KiB CABAC slice equivalence test */
 static unsigned char big_rbsp[65536],big_nal[70000],big_fast[70000],big_full[70000];
 struct poc_out bhdr={big_rbsp,sizeof big_rbsp,0,0};
 poc_put(&bhdr,1);poc_put(&bhdr,0);poc_put(&bhdr,1);poc_put(&bhdr,1);poc_put(&bhdr,1);
 for(unsigned k=poc_state.log2_frame_num;k--;)poc_put(&bhdr,0);
 poc_put(&bhdr,1);
 for(unsigned k=poc_state.log2_poc_lsb;k--;)poc_put(&bhdr,(k&1));
 poc_put(&bhdr,0);poc_put(&bhdr,0);poc_put(&bhdr,1);
 if(poc_state.deblocking){poc_put(&bhdr,1);poc_put(&bhdr,1);poc_put(&bhdr,1);}
 while(bhdr.bits&7)poc_put(&bhdr,1);
 size_t bhbytes=bhdr.bits/8;
 for(size_t i=bhbytes;i<sizeof big_rbsp;i++)big_rbsp[i]=(unsigned char)((i%19==0)?0:(i*31+7));
 big_nal[0]=0x65;
 size_t besc=poc_escape(big_rbsp,sizeof big_rbsp,big_nal+1,sizeof big_nal-1);
 assert(besc>0);
 size_t bfast=poc_rewrite_slice(big_nal,besc+1,big_fast,sizeof big_fast);
 size_t bfull=poc_rewrite_slice_full(big_nal,besc+1,big_full,sizeof big_full);
 assert(bfast>0&&bfast==bfull&&!memcmp(big_fast,big_full,bfast));
 /* Reset state so following tests start clean. */
 poc_state.active=0;poc_state.pps_ok=0;
}
int main(void){
 check_poc_bit_copy();
 check_poc_slice_fast_equivalence();
 assert(video_window(0)==1&&video_window(1)==1&&video_window(8)==8&&video_window(4096)==32);
 unsigned char sc[]={0x00,0x00,0x02,0x00,0x00,0x00,0x00,0x01,0x65,0x00,0x00,0x01,0x41,0x00,0x00};
 assert(annexb_next(sc,sizeof sc,0)==4);
 assert(annexb_next(sc,sizeof sc,8)==9);
 assert(annexb_next(sc,sizeof sc,12)==sizeof sc);
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
