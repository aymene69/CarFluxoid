/* SPDX-License-Identifier: GPL-3.0-or-later
 * Rewrite a frame-only H.264 stream from pic_order_cnt_type 0 to type 2 without re-encoding.
 * The Uconnect i.MX6 VPU holds POC type 0 pictures back for reordering, so the iPhone stream
 * (no B frames, max_num_reorder_frames 0) never reaches the display and the decoder is reset.
 * Only header bits move: the SPS gets POC type 2, each slice loses pic_order_cnt_lsb, and the
 * byte-aligned CABAC payload is copied unchanged. Unsupported streams are left untouched.
 */
#ifndef CARFLUXOID_H264_POC_H
#define CARFLUXOID_H264_POC_H
#include <stddef.h>
#include <stdint.h>
#include <string.h>
struct poc_bits {const unsigned char *p;size_t bits,i;int err;};
static unsigned poc_bit(struct poc_bits *b){if(b->i>=b->bits){b->err=1;return 0;}unsigned v=b->p[b->i>>3]>>(7-(b->i&7))&1;b->i++;return v;}
static uint32_t poc_u(struct poc_bits *b,unsigned n){uint32_t v=0;while(n--)v=v<<1|poc_bit(b);return v;}
static uint32_t poc_ue(struct poc_bits *b){unsigned z=0;while(!poc_bit(b)&&!b->err)if(++z>31){b->err=1;return 0;}return (uint32_t)((1ULL<<z)-1+poc_u(b,z));}
static void poc_se(struct poc_bits *b){poc_ue(b);}
struct poc_out {unsigned char *p;size_t cap,bits;int err;};
static void poc_put(struct poc_out *o,unsigned v){if(o->bits>=o->cap*8){o->err=1;return;}if(!(o->bits&7))o->p[o->bits>>3]=0;if(v)o->p[o->bits>>3]|=(unsigned char)(0x80>>(o->bits&7));o->bits++;}
static void poc_copy(struct poc_out *o,const struct poc_bits *b,size_t from,size_t to){
 if(from>to||to>b->bits||o->bits>o->cap*8||to-from>o->cap*8-o->bits){
  o->err=1;
  return;
 }
 while(from<to&&(o->bits&7)){
  poc_put(o,b->p[from>>3]>>(7-(from&7))&1);
  from++;
 }
 size_t bytes=(to-from)/8;
 unsigned shift=from&7;
 const unsigned char *source=b->p+from/8;
 unsigned char *target=o->p+o->bits/8;
 if(bytes&&!shift)memcpy(target,source,bytes);
 else for(size_t i=0;i<bytes;i++)
  target[i]=(unsigned char)((source[i]<<shift)|(source[i+1]>>(8-shift)));
 from+=bytes*8;
 o->bits+=bytes*8;
 while(from<to){
  poc_put(o,b->p[from>>3]>>(7-(from&7))&1);
  from++;
 }
}
static size_t poc_unescape(const unsigned char *in,size_t n,unsigned char *out){
 size_t k=0,zeros=0;for(size_t i=0;i<n;i++){if(zeros>=2&&in[i]==3){zeros=0;continue;}out[k++]=in[i];zeros=in[i]?0:zeros+1;}return k;
}
static size_t poc_escape(const unsigned char *in,size_t n,unsigned char *out,size_t cap){
 size_t k=0,zeros=0;for(size_t i=0;i<n;i++){if(zeros>=2&&in[i]<=3){if(k>=cap)return 0;out[k++]=3;zeros=0;}if(k>=cap)return 0;out[k++]=in[i];zeros=in[i]?0:zeros+1;}return k;
}
static size_t poc_stop_bit(const unsigned char *p,size_t n){
 while(n&&!p[n-1])n--;if(!n)return 0;unsigned last=p[n-1],t=0;while(!(last>>t&1))t++;return n*8-1-t;
}
static struct {int active;unsigned log2_frame_num,log2_poc_lsb,separate_plane,pps_id,cabac,bottom_poc,redundant,weighted_p,weighted_b,deblocking;int pps_ok;} poc_state;
static unsigned char poc_rbsp[1<<20],poc_tmp[1<<20];

static void poc_hrd(struct poc_bits *b){
 uint32_t count=poc_ue(b)+1;poc_u(b,8);for(uint32_t i=0;i<count&&i<32&&!b->err;i++){poc_ue(b);poc_ue(b);poc_bit(b);}poc_u(b,20);
}
/* POC type 2 forbids reordering: require the encoder's explicit max_num_reorder_frames 0. */
static int poc_no_reorder(struct poc_bits *b){
 poc_bit(b);if(poc_bit(b)){poc_ue(b);poc_ue(b);poc_ue(b);poc_ue(b);}if(!poc_bit(b))return 0;
 if(poc_bit(b)&&poc_u(b,8)==255)poc_u(b,32);
 if(poc_bit(b))poc_bit(b);
 if(poc_bit(b)){poc_u(b,4);if(poc_bit(b))poc_u(b,24);}
 if(poc_bit(b)){poc_ue(b);poc_ue(b);}
 if(poc_bit(b)){poc_u(b,32);poc_u(b,32);poc_bit(b);}
 int nal_hrd=poc_bit(b);if(nal_hrd)poc_hrd(b);int vcl_hrd=poc_bit(b);if(vcl_hrd)poc_hrd(b);if(nal_hrd||vcl_hrd)poc_bit(b);
 poc_bit(b);if(!poc_bit(b))return 0;
 poc_bit(b);poc_ue(b);poc_ue(b);poc_ue(b);poc_ue(b);return !poc_ue(b)&&!b->err;
}
/* Returns the rewritten SPS NAL length, or 0 to keep the original. */
static size_t poc_rewrite_sps(const unsigned char *nal,size_t n,unsigned char *out,size_t cap,int force){
 poc_state.active=0;if(n<4||n>sizeof poc_rbsp)return 0;
 size_t len=poc_unescape(nal+1,n-1,poc_rbsp);struct poc_bits b={poc_rbsp,len*8,0,0};
 unsigned profile=poc_u(&b,8);poc_u(&b,16);poc_ue(&b);poc_state.separate_plane=0;
 if(profile==100||profile==110||profile==122||profile==244||profile==44||profile==83||profile==86||profile==118||profile==128||profile==138||profile==139||profile==134||profile==135){
  if(poc_ue(&b)==3)poc_state.separate_plane=poc_bit(&b);poc_ue(&b);poc_ue(&b);poc_bit(&b);if(poc_bit(&b))return 0;
 }
 poc_state.log2_frame_num=poc_ue(&b)+4;size_t type_at=b.i;if(poc_ue(&b)!=0||b.err)return 0;
 size_t lsb_at=b.i;poc_state.log2_poc_lsb=poc_ue(&b)+4;size_t rest=b.i;
 poc_ue(&b);poc_bit(&b);poc_ue(&b);poc_ue(&b);if(!poc_bit(&b)||b.err)return 0; /* frame_mbs_only */
 if(!poc_no_reorder(&b)&&!force)return 0;
 size_t stop=poc_stop_bit(poc_rbsp,len);if(stop<rest)return 0;
 struct poc_out o={poc_tmp,sizeof poc_tmp,0,0};
 poc_copy(&o,&b,0,type_at);poc_put(&o,0);poc_put(&o,1);poc_put(&o,1); /* ue(2) */
 (void)lsb_at;poc_copy(&o,&b,rest,stop);poc_put(&o,1);while(o.bits&7)poc_put(&o,0);
 if(o.err||cap<1)return 0;out[0]=nal[0];size_t k=poc_escape(poc_tmp,o.bits/8,out+1,cap-1);if(!k)return 0;
 poc_state.active=1;poc_state.pps_ok=0;return k+1;
}
static void poc_parse_pps(const unsigned char *nal,size_t n){
 poc_state.pps_ok=0;if(n<2||n>sizeof poc_rbsp)return;
 size_t len=poc_unescape(nal+1,n-1,poc_rbsp);struct poc_bits b={poc_rbsp,len*8,0,0};
 poc_state.pps_id=poc_ue(&b);poc_ue(&b);poc_state.cabac=poc_bit(&b);poc_state.bottom_poc=poc_bit(&b);
 if(poc_ue(&b))return; /* slice groups */
 poc_ue(&b);poc_ue(&b);poc_state.weighted_p=poc_bit(&b);poc_state.weighted_b=poc_u(&b,2);poc_se(&b);poc_se(&b);poc_se(&b);
 poc_state.deblocking=poc_bit(&b);poc_bit(&b);poc_state.redundant=poc_bit(&b);
 poc_state.pps_ok=!b.err&&!poc_state.bottom_poc;
}
static int poc_ref_list_modification(struct poc_bits *b){
 if(!poc_bit(b))return 0;for(unsigned guard=0;guard<64;guard++){uint32_t idc=poc_ue(b);if(idc==3)return 0;if(idc>3)return -1;poc_ue(b);}return -1;
}
/* Returns the rewritten slice NAL length, or 0 to keep the original. */
static size_t poc_rewrite_slice(const unsigned char *nal,size_t n,unsigned char *out,size_t cap){
 if(!poc_state.active||!poc_state.pps_ok||n<2||n>sizeof poc_rbsp)return 0;
 unsigned ref_idc=nal[0]>>5&3,idr=(nal[0]&31)==5;
 size_t len=poc_unescape(nal+1,n-1,poc_rbsp);struct poc_bits b={poc_rbsp,len*8,0,0};
 poc_ue(&b);unsigned type=poc_ue(&b)%5;if(poc_ue(&b)!=poc_state.pps_id)return 0;
 if(type==1){poc_state.active=0;return 0;} /* B slices would need the original picture order */
 if(poc_state.separate_plane)poc_u(&b,2);poc_u(&b,poc_state.log2_frame_num);if(idr)poc_ue(&b);
 size_t lsb_from=b.i;poc_u(&b,poc_state.log2_poc_lsb);size_t lsb_to=b.i;
 if(poc_state.redundant)poc_ue(&b);
 if(type==1)poc_bit(&b);
 if(type==0||type==3||type==1){if(poc_bit(&b)){poc_ue(&b);if(type==1)poc_ue(&b);}}
 if(type!=2&&type!=4){if(poc_ref_list_modification(&b))return 0;if(type==1&&poc_ref_list_modification(&b))return 0;}
 if((poc_state.weighted_p&&(type==0||type==3))||(poc_state.weighted_b==1&&type==1))return 0;
 if(ref_idc){
  if(idr){poc_bit(&b);poc_bit(&b);}
  else if(poc_bit(&b))for(unsigned guard=0;;guard++){uint32_t op=poc_ue(&b);if(!op)break;if(guard>=64||op>6)return 0;if(op!=5)poc_ue(&b);if(op==3)poc_ue(&b);}
 }
 if(poc_state.cabac&&type!=2&&type!=4)poc_ue(&b);
 poc_se(&b);
 if(type==3||type==4){if(type==3)poc_bit(&b);poc_se(&b);}
 if(poc_state.deblocking&&poc_ue(&b)!=1){poc_se(&b);poc_se(&b);}
 if(b.err)return 0;
 struct poc_out o={poc_tmp,sizeof poc_tmp,0,0};
 poc_copy(&o,&b,0,lsb_from);poc_copy(&o,&b,lsb_to,b.i);
 if(poc_state.cabac){
  while(b.i&7)if(!poc_bit(&b))return 0; /* cabac_alignment_one_bit */
  while(o.bits&7)poc_put(&o,1);
  if(o.err||o.bits/8+len-b.i/8>sizeof poc_tmp)return 0;
  memcpy(poc_tmp+o.bits/8,poc_rbsp+b.i/8,len-b.i/8);o.bits+=(len-b.i/8)*8;
 }else{
  size_t stop=poc_stop_bit(poc_rbsp,len);if(stop<b.i)return 0;
  poc_copy(&o,&b,b.i,stop);poc_put(&o,1);while(o.bits&7)poc_put(&o,0);
 }
 if(o.err||cap<1)return 0;out[0]=nal[0];size_t k=poc_escape(poc_tmp,o.bits/8,out+1,cap-1);return k?k+1:0;
}
#endif
