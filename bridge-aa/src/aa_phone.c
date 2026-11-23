/* SPDX-License-Identifier: GPL-3.0-or-later
 * Minimal phone-side Android Auto test sender. No CarPlay bridge yet.
 * Protocol definitions checked against LIVI and AARE (see README).
 */
#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <signal.h>
#include <pthread.h>
#include <sched.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/select.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>
#include <sys/syscall.h>
#include <linux/aio_abi.h>
#include <mbedtls/ssl.h>
#include <mbedtls/entropy.h>
#include <mbedtls/ctr_drbg.h>
#include <mbedtls/error.h>
#include <mbedtls/net_sockets.h>

#include "mtp.h"
#include "h264_poc.h"

#define CAP 65536
static int buffered_test;
static int diagnostic;
#define TRACE(...) do { if(diagnostic) fprintf(stderr,__VA_ARGS__); } while(0)
static int fd, out_fd, usb_mode, ep0=-1, handshake=1, rx_ch, tx_ch, tx_flags=11;
static unsigned char usb_buffer[CAP];
static size_t usb_pos,usb_len;
static unsigned char wire[CAP], plain[CAP], incoming[CAP];
static size_t left, at;
static volatile sig_atomic_t stop;
static mbedtls_ssl_context ssl;
static FILE *raw_capture,*frame_capture;
static void capture_frame(int ch,int flags,const unsigned char *p,size_t n){
 if(!frame_capture)return;
 unsigned char h[4]={(unsigned char)ch,(unsigned char)flags,(unsigned char)(n>>8),(unsigned char)n};
 fwrite(h,1,4,frame_capture);fwrite(p,1,n,frame_capture);fflush(frame_capture);
}

static uint64_t now_us(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return (uint64_t)t.tv_sec*1000000+t.tv_nsec/1000;}
/* Persist the AA sequence line by line: a head unit may cut VBUS right after a failure. */
static int trace_fd=-1;static unsigned trace_lines;static uint64_t trace_start;
static void session_trace(const char *fmt,...){
 if(trace_fd<0||trace_lines>=2000)return;
 char line[200];va_list ap;int n=snprintf(line,sizeof line,"%llu ",(unsigned long long)((now_us()-trace_start)/1000));
 va_start(ap,fmt);int m=vsnprintf(line+n,sizeof line-(size_t)n-1,fmt,ap);va_end(ap);
 if(m>0)n+=m;if(n>(int)sizeof line-2)n=(int)sizeof line-2;line[n++]='\n';
 if(write(trace_fd,line,(size_t)n)==n)fsync(trace_fd);trace_lines++;
}
static void open_session_trace(void){
 const char *path=getenv("AA_SESSION_TRACE");if(!path||!*path)return;
 struct stat st;if(!stat(path,&st)&&st.st_size>65536){char old[4096];snprintf(old,sizeof old,"%s.old",path);rename(path,old);}
 trace_fd=open(path,O_WRONLY|O_CREAT|O_APPEND|O_CLOEXEC,0600);trace_start=now_us();
}
static const char *hex16(const unsigned char *p,size_t n){
 static char s[33];size_t k=0;for(size_t i=0;i<n&&i<16;i++)k+=(size_t)snprintf(s+k,sizeof s-k,"%02x",p[i]);s[k]=0;return s;
}
static const char *hex64(const unsigned char *p,size_t n){
 static char s[129];size_t k=0;for(size_t i=0;i<n&&i<64;i++)k+=(size_t)snprintf(s+k,sizeof s-k,"%02x",p[i]);s[k]=0;return s;
}
static void die(const char *s){session_trace("FATAL %s errno=%d",s,errno);perror(s);exit(1);}
static void interrupted(int s){(void)s;stop=1;}
static int all(int write_mode,void *buf,size_t n){
 unsigned char *p=buf;size_t done=0;
 while(done<n&&!stop){
  ssize_t r;
  if(!write_mode&&usb_mode){
   if(usb_pos==usb_len){r=read(fd,usb_buffer,sizeof usb_buffer);if(r<0&&errno==EINTR)continue;if(r==0){fprintf(stderr,"USB reader stream closed\n");return -1;}if(r<0){fprintf(stderr,"USB OUT read failed: errno=%d (%s) stop=%d\n",errno,strerror(errno),(int)stop);return -1;}usb_len=(size_t)r;usb_pos=0;}
   size_t take=n-done;if(take>usb_len-usb_pos)take=usb_len-usb_pos;
   memcpy(p+done,usb_buffer+usb_pos,take);usb_pos+=take;r=(ssize_t)take;
  }else r=write_mode?write(out_fd,p+done,n-done):read(fd,p+done,n-done);
  if(r<0&&errno==EINTR)continue;
  if(r<=0){if(write_mode&&usb_mode)fprintf(stderr,"USB IN write failed: errno=%d (%s) len=%zu\n",r<0?errno:0,r<0?strerror(errno):"eof",n-done);return -1;}
  done+=(size_t)r;
 }
 return done==n?0:-1;
}
static uint32_t fragmented_total;
static int frame(int ch,int flags,const unsigned char *p,size_t n){
 if(n>65535)return -1;
 unsigned char packet[CAP+8]={(unsigned char)ch,(unsigned char)flags,(unsigned char)(n>>8),(unsigned char)n};
 size_t header=4;
 if((flags&3)==1){packet[4]=fragmented_total>>24;packet[5]=fragmented_total>>16;packet[6]=fragmented_total>>8;packet[7]=fragmented_total;header=8;}
 memcpy(packet+header,p,n);
 if(handshake)fprintf(stderr,"AA TX handshake ch=%d flags=%02x bytes=%zu\n",ch,flags,n);
 return all(1,packet,n+header);
}
static int receive(int *ch,int *flags,unsigned char *p,size_t *n){
 unsigned char h[4];if(all(0,h,4))return -1;
 *ch=h[0];*flags=h[1];*n=((size_t)h[2]<<8)|h[3];
 /* This first prototype accepts complete AA messages only. */
 if((*flags&3)!=3){fprintf(stderr,"Fragmented input unsupported: flags=%02x\n",*flags);return -1;}
 int result=all(0,p,*n);if(!result)capture_frame(*ch,*flags,p,*n);
 if(diagnostic&&!handshake&&result==0){
  static unsigned sequence;static uint32_t previous;uint32_t hash=2166136261u;
  for(size_t k=0;k<*n;k++)hash=(hash^p[k])*16777619u;
  fprintf(stderr,"AA RX frame=%u ch=%d flags=%02x bytes=%zu fingerprint=%08x%s TLShead=",++sequence,*ch,*flags,*n,hash,hash==previous?" REPEATED":"");
  for(size_t k=0;k<*n&&k<13;k++)fprintf(stderr,"%02x",p[k]);
  fprintf(stderr,"\n");previous=hash;
 }
 if(handshake)fprintf(stderr,"AA RX handshake ch=%d flags=%02x bytes=%zu result=%d\n",*ch,*flags,*n,result);return result;
}
static unsigned char handshake_flight[CAP];
static size_t handshake_used;
static int flush_handshake(void){
 if(!handshake_used)return 0;
 wire[0]=0;wire[1]=3;memcpy(wire+2,handshake_flight,handshake_used);
 int result=frame(0,3,wire,handshake_used+2);handshake_used=0;return result;
}
static int tls_send(void *ctx,const unsigned char *buf,size_t n){
 (void)ctx;
 if(handshake){if(handshake_used+n+2>CAP)return MBEDTLS_ERR_SSL_INTERNAL_ERROR;memcpy(handshake_flight+handshake_used,buf,n);handshake_used+=n;}
 else if(frame(tx_ch,tx_flags,buf,n))return MBEDTLS_ERR_NET_SEND_FAILED;
 return (int)n;
}
static int tls_recv(void *ctx,unsigned char *buf,size_t n){
 (void)ctx;
 if(handshake&&flush_handshake())return MBEDTLS_ERR_NET_SEND_FAILED;
 if(!left){int flags;size_t len;if(receive(&rx_ch,&flags,incoming,&len))return MBEDTLS_ERR_NET_RECV_FAILED;
  if(!(flags&8)){if(len<2||incoming[0]!=0||incoming[1]!=3)return MBEDTLS_ERR_SSL_UNEXPECTED_MESSAGE;at=2;left=len-2;}
  else {at=0;left=len;}
 }
 if(n>left)n=left;memcpy(buf,incoming+at,n);at+=n;left-=n;return (int)n;
}
static void check(int r,const char *label){if(r<0){char b[200];mbedtls_strerror(r,b,sizeof b);session_trace("FATAL %s -%04x",label,-r);fprintf(stderr,"%s: %s (-%04x)\n",label,b,-r);exit(1);}}
/* Unit tests capture outgoing messages here; production never sets it. */
static void (*send_hook)(int ch,int id,const void *data,size_t n);
static void send_msg(int ch,int flags,int id,const void *data,size_t n){
 if(send_hook){send_hook(ch,id,data,n);return;}
 if(n+2>16000){fprintf(stderr,"Message exceeds prototype TLS record limit\n");exit(1);}
 plain[0]=(unsigned char)(id>>8);plain[1]=(unsigned char)id;if(data!=plain+2&&n)memmove(plain+2,data,n);tx_ch=ch;tx_flags=flags;
 int r=mbedtls_ssl_write(&ssl,plain,n+2);check(r,"TLS write");if((size_t)r!=n+2){fprintf(stderr,"Short TLS write\n");exit(1);}
}
/* Bounded protobuf reader: enough to select channels from ServiceDiscovery. */
static int varint(const unsigned char *p,size_t n,size_t *i,uint64_t *v){
 *v=0;for(unsigned j=0;j<10&&*i<n;j++){unsigned b=p[(*i)++];if(j==9&&b>1)return -1;*v|=(uint64_t)(b&127)<<(7*j);if(!(b&128))return 0;}return -1;
}
static int field(const unsigned char *p,size_t n,size_t *i,unsigned *tag,uint64_t *v,const unsigned char **b,size_t *len){
 uint64_t key;if(varint(p,n,i,&key)||key<8)return -1;*tag=(unsigned)(key>>3);*b=NULL;*len=0;
 switch(key&7){case 0:return varint(p,n,i,v);case 2:if(varint(p,n,i,v)||*v>n-*i)return -1;*len=(size_t)*v;*b=p+*i;*i+=*len;return 0;case 1:if(n-*i<8)return -1;*i+=8;return 0;case 5:if(n-*i<4)return -1;*i+=4;return 0;default:return -1;}
}
static uint64_t value(const unsigned char *p,size_t n,unsigned wanted,uint64_t fallback){
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return fallback;if(t==wanted&&!b)return v;}return fallback;
}
static int selected_codec=3;
static unsigned display_width=800,display_height=480;

static unsigned char *waiting_video;
static size_t waiting_size;
static void load_waiting_video(void){
 const char *dir=getenv("AA_WAIT_SCREEN_DIR");if(!dir)return;
 char path[4096];snprintf(path,sizeof path,"%s/attente-%ux%u.h264",dir,display_width,display_height);
 FILE *file=fopen(path,"rb");if(!file){fprintf(stderr,"WAIT_SCREEN unavailable %s\n",path);return;}
 if(fseek(file,0,SEEK_END)){fclose(file);return;}long n=ftell(file);rewind(file);
 if(n<=0||n>1024*1024){fclose(file);return;}
 waiting_video=malloc((size_t)n);if(!waiting_video){fclose(file);return;}
 waiting_size=fread(waiting_video,1,(size_t)n,file);fclose(file);
 if(waiting_size!=(size_t)n){free(waiting_video);waiting_video=NULL;waiting_size=0;return;}
 fprintf(stderr,"WAIT_SCREEN ready %ux%u bytes=%zu\n",display_width,display_height,waiting_size);
}
static int selected_video_config;
static unsigned video_margin_w,video_margin_h;
static unsigned requested_resolution(void){
 const char *path=getenv("AA_SETTINGS_FILE");if(!path)path="/persist/aa-settings/options.txt";
 FILE *f=fopen(path,"r");if(!f)return 0;char line[256];unsigned cap=0;
 while(fgets(line,sizeof line,f)){unsigned v;if(sscanf(line,"max_resolution=%u",&v)==1&&(v==0||v==480||v==720||v==1080))cap=v;}
 fclose(f);return cap;
}
/* h264_poc2=0 in the settings file keeps the iPhone POC type 0 stream unchanged. */
static int h264_poc2_enabled(void){
 const char *path=getenv("AA_SETTINGS_FILE");if(!path)path="/persist/aa-settings/options.txt";
 FILE *f=fopen(path,"r");if(!f)return 1;char line[256];unsigned enabled=1;
 while(fgets(line,sizeof line,f)){unsigned v;if(sscanf(line,"h264_poc2=%u",&v)==1&&v<=1)enabled=v;}
 fclose(f);return (int)enabled;
}
static int h264_poc2=1;
static int find_video(const unsigned char *p,size_t n){
 unsigned cap=requested_resolution();size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 selected_video_config=0;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return -1;if(t!=1||!b)continue;
  int id=(int)value(b,l,1,0);size_t j=0,sl;unsigned st;uint64_t sv;const unsigned char *sb;
  while(j<l){if(field(b,l,&j,&st,&sv,&sb,&sl))return -1;if(st!=3||!sb)continue;
   int codec=(int)value(sb,sl,1,0),index=0,choice=-1,smallest=-1;unsigned chosen_res=0,smallest_res=9999;
   size_t ci=0,cl;unsigned ct;uint64_t cv;const unsigned char *cb;
   while(ci<sl){if(field(sb,sl,&ci,&ct,&cv,&cb,&cl))return -1;if(ct!=4||!cb)continue;
    unsigned r=(unsigned)value(cb,cl,1,1),height=r==1?480:r==2?720:r==3?1080:0;
    if(index<128&&height){
     if(height<smallest_res){smallest=index;smallest_res=height;}
     if((cap==0&&choice<0)||(cap&&height<=cap&&height>chosen_res)){choice=index;chosen_res=height;}
    }index++;
   }
   if(!(codec==3||codec==7||(codec==0&&index)))continue;
   if(getenv("AA_VIDEO_SOCKET")&&codec==7)continue; /* CarPlay passthrough is H.264. */
   if(choice<0){choice=smallest;chosen_res=smallest_res;}
   if(index&&choice<0)continue;
   if(choice<0){choice=0;chosen_res=480;}
   if(id<=0||id>=256)continue;
   selected_video_config=choice;selected_codec=codec?codec:3;video_margin_w=video_margin_h=0;
   /* Margins of the chosen configuration: the head unit hides them (Uconnect status bar). */
   for(size_t mi=0,cfg=0;mi<sl;){if(field(sb,sl,&mi,&ct,&cv,&cb,&cl))break;if(ct!=4||!cb)continue;if((int)cfg++==choice){video_margin_w=(unsigned)value(cb,cl,3,0);video_margin_h=(unsigned)value(cb,cl,4,0);break;}}
   display_height=chosen_res;display_width=chosen_res==480?800:chosen_res==720?1280:1920;
   fprintf(stderr,"AA_VIDEO_SELECTED channel=%d codec=%d config=%d size=%ux%u requested_cap=%u\n",id,selected_codec,choice,display_width,display_height,cap);return id;
  }
 }return -1;
}
static int input_channel=-1,audio_channel=-1,audio_fd=-1,audio_ready,audio_unacked,audio_max_unacked=1;
/* CarPlay media -> AA media sink, in the order of Google's head unit integration guide: audio focus
 * GAIN first, then START with a new session, then PCM, and STOP once the iPhone goes quiet.
 * The Uconnect routes AA media from its playbackStart callback (CVTOPROJECTIONDEVICE.DLL,
 * AASinkCb:playbackStartCallback). START used to leave at channel setup, before any focus, and PCM
 * before the grant: the sink acknowledged every block (car log 2026-11-18) but routed none to the
 * speakers. CONFIG is parsed before any reply: replies reuse the receive buffer. */
#define AUDIO_WINDOW_MAX 4096
#define VIDEO_WINDOW_MAX 32
#define AUDIO_MAX_LATE_US 80000ULL
#define AUDIO_FOCUS_RETRY_US 2000000ULL
#define AUDIO_STOP_IDLE_US 2000000ULL
#define AUDIO_RELEASE_IDLE_US 15000000ULL
static int audio_window(uint64_t announced){return announced<1?1:announced>AUDIO_WINDOW_MAX?AUDIO_WINDOW_MAX:(int)announced;}
static int video_window(uint64_t announced){return announced<1?1:announced>VIDEO_WINDOW_MAX?VIDEO_WINDOW_MAX:(int)announced;}
static unsigned parse_audio_preroll(const char *s){
 if(!s||!*s)return 0;
 char *end=NULL;unsigned long v=strtoul(s,&end,10);
 if(end==s||*end)return 0;
 if(v==0)return 0;
 if(v==1)return 60000;
 if(v<10)v=10;if(v>600)v=600;
 return (unsigned)(v*1000);
}
static unsigned audio_sent,audio_acks,audio_underflows,audio_stalls,audio_peak,audio_discarded,audio_session,ping_seen;
static int audio_streaming;
static uint64_t audio_full_at,audio_stats_at,audio_pump_at,audio_pump_gap_max;
/* audio_focus: 0 none, 1 requested, 2 granted (GAIN, GAIN_TRANSIENT, LOSS_TRANSIENT_CAN_DUCK, GAIN_MEDIA_ONLY). */
static int audio_focus;static uint64_t audio_focus_at,audio_last;
static uint64_t audio_samples,audio_epoch,audio_prime_until;
static unsigned audio_preroll_us;
static void request_audio_focus(unsigned type,uint64_t now){unsigned char req[]={8,(unsigned char)type};send_msg(0,11,0x12,req,sizeof req);audio_focus_at=now;session_trace("TX audio_focus_request type=%u",type);}
static void audio_on_config(const unsigned char *body,size_t len){
 if(value(body,len,1,0)!=2){session_trace("AUDIO_CONFIG rejected %s",hex16(body,len));return;}
 audio_max_unacked=audio_window(value(body,len,2,1));audio_unacked=0;audio_ready=1;
 fprintf(stderr,"AUDIO_READY 48000 stereo PCM16 window=%d\n",audio_max_unacked);session_trace("AUDIO_READY window=%d",audio_max_unacked);
}
static int media_status_channel=-1,media_status_ready;
static int find_media_status(const unsigned char *p,size_t n){
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return -1;if(t!=1||!b)continue;
  int id=(int)value(b,l,1,0);size_t j=0,sl;unsigned st;uint64_t sv;const unsigned char *sb;
  while(j<l){if(field(b,l,&j,&st,&sv,&sb,&sl))return -1;if(st==9&&sb&&id>0&&id<256)return id;}
 }return -1;
}
static int sensor_channel=-1,car_night_mode=-1;
static uint32_t sensor_mask;
static unsigned sensor_gps_count,sensor_speed_count;
static int find_sensor(const unsigned char *p,size_t n,uint32_t *mask){
 *mask=0;
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return -1;if(t!=1||!b)continue;
  int id=(int)value(b,l,1,0);size_t j=0,sl;unsigned st;uint64_t sv;const unsigned char *sb;
  while(j<l){if(field(b,l,&j,&st,&sv,&sb,&sl))return -1;
   if(st==2&&sb&&id>0&&id<256){
    size_t k=0,kl;unsigned kt;uint64_t kv;const unsigned char *kb;
    while(k<sl){if(field(sb,sl,&k,&kt,&kv,&kb,&kl))break;
     if(kt==1&&kb){uint64_t kind=value(kb,kl,1,0);if(kind>=1&&kind<=31)*mask|=1u<<(unsigned)kind;}
    }
    return id;
   }
  }
 }return -1;
}
static size_t encode_sensor_request(unsigned char *out,unsigned type,int64_t period_ms){
 size_t n=0;
 out[n++]=0x08;
 while(type>=128){out[n++]=(unsigned char)((type&127)|128);type>>=7;}
 out[n++]=(unsigned char)type;
 out[n++]=0x10;
 uint64_t p=(uint64_t)period_ms;
 while(p>=128){out[n++]=(unsigned char)((p&127)|128);p>>=7;}
 out[n++]=(unsigned char)p;
 return n;
}
static void sensor_subscribe(void){
 if(sensor_channel<=0)return;
 static const struct { unsigned type; int64_t period_ms; } wants[]={
  {10,0},    /* SENSOR_NIGHT_MODE */
  {1,1000},  /* SENSOR_LOCATION */
  {3,1000},  /* SENSOR_SPEED */
 };
 unsigned char req[16];
 for(size_t i=0;i<sizeof wants/sizeof *wants;i++){
  if(!(sensor_mask&(1u<<wants[i].type)))continue;
  size_t len=encode_sensor_request(req,wants[i].type,wants[i].period_ms);
  send_msg(sensor_channel,11,0x8001,req,len);
 }
 fprintf(stderr,"AA_SENSOR_SUB ch=%d mask=0x%x\n",sensor_channel,sensor_mask);
 session_trace("AA_SENSOR_SUB ch=%d mask=0x%x",sensor_channel,sensor_mask);
}
static void put_le32(unsigned char *dst,uint32_t v){
 dst[0]=(unsigned char)(v&255);dst[1]=(unsigned char)((v>>8)&255);
 dst[2]=(unsigned char)((v>>16)&255);dst[3]=(unsigned char)((v>>24)&255);
}
static int find_bluetooth_mac(const unsigned char *p,size_t n,char out[18]){
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return 0;if(t!=1||!b)continue;
  size_t j=0,sl;unsigned st;uint64_t sv;const unsigned char *sb;
  while(j<l){if(field(b,l,&j,&st,&sv,&sb,&sl))return 0;
   if(st==6&&sb){
    size_t k=0,kl;unsigned kt;uint64_t kv;const unsigned char *kb;
    while(k<sl){if(field(sb,sl,&k,&kt,&kv,&kb,&kl))break;
     if(kt==1&&kb&&kl==17){
      int ok=1;
      for(size_t m=0;m<17;m++){
       unsigned char c=kb[m];
       if(m%3==2){if(c!=':'){ok=0;break;}out[m]=':';}
       else if(c>='0'&&c<='9')out[m]=(char)c;
       else if(c>='a'&&c<='f')out[m]=(char)(c-'a'+'A');
       else if(c>='A'&&c<='F')out[m]=(char)c;
       else{ok=0;break;}
      }
      if(ok){out[17]=0;return 1;}
     }
    }
   }
  }
 }
 return 0;
}
static int pb_put_varint(unsigned char *out,size_t cap,size_t *pos,uint64_t val){
 unsigned char tmp[10];size_t n=0;
 do{tmp[n++]=(unsigned char)((val&127)|(val>=128?128:0));val>>=7;}while(val);
 if(*pos+n>cap)return -1;
 memcpy(out+*pos,tmp,n);*pos+=n;return 0;
}
static int pb_put_field_varint(unsigned char *out,size_t cap,size_t *pos,unsigned tag,uint64_t val){
 return pb_put_varint(out,cap,pos,((uint64_t)tag<<3)|0)||pb_put_varint(out,cap,pos,val);
}
static int pb_put_field_bytes(unsigned char *out,size_t cap,size_t *pos,unsigned tag,const unsigned char *s,size_t len){
 if(!len)return 0;
 if(pb_put_varint(out,cap,pos,((uint64_t)tag<<3)|2)||pb_put_varint(out,cap,pos,len))return -1;
 if(*pos+len>cap)return -1;
 memcpy(out+*pos,s,len);*pos+=len;return 0;
}
static unsigned char media_meta_buf[512];
static size_t media_meta_len;
static unsigned media_elapsed_secs;
static char media_source_name[64]="CarPlay";
static void send_default_media_meta(void){
 if(media_status_channel<=0||!media_status_ready||media_meta_len)return;
 unsigned char msg[64];size_t pos=0;
 const char *song="CarPlay",*artist="CarFluxoid";
 if(pb_put_field_bytes(msg,sizeof msg,&pos,1,(const unsigned char*)song,strlen(song)))return;
 if(pb_put_field_bytes(msg,sizeof msg,&pos,2,(const unsigned char*)artist,strlen(artist)))return;
 send_msg(media_status_channel,11,0x8003,msg,pos);
 session_trace("AA_MEDIA_META_DEFAULT ch=%d bytes=%zu",media_status_channel,pos);
}
static void media_status_update(unsigned state){
 if(media_status_channel<=0||!media_status_ready)return;
 if(state==2&&!media_meta_len)send_default_media_meta();
 /* MediaPlaybackStatus (0x8001): field 1 = state (1=STOPPED, 2=PLAYING, 3=PAUSED), field 2 = source, field 3 = playback_seconds. */
 unsigned char msg[96];size_t pos=0;
 const char *src=media_source_name[0]?media_source_name:"CarPlay";
 if(pb_put_field_varint(msg,sizeof msg,&pos,1,state))return;
 if(pb_put_field_bytes(msg,sizeof msg,&pos,2,(const unsigned char*)src,strlen(src)))return;
 if(pb_put_field_varint(msg,sizeof msg,&pos,3,media_elapsed_secs))return;
 send_msg(media_status_channel,11,0x8001,msg,pos);
 session_trace("AA_MEDIA_STATUS ch=%d state=%u pos=%us src=%s",media_status_channel,state,media_elapsed_secs,src);
}
/* Decode compact 0x30 NowPlaying datagram into AA MediaPlaybackMetadata (0x8003) protobuf. */
static size_t encode_media_metadata(unsigned char *out,size_t cap,const unsigned char *pkt,size_t plen,unsigned *out_state,char app_out[64]){
 if(plen<8||pkt[0]!=0x30)return 0;
 unsigned state=pkt[1];
 unsigned duration=(unsigned)pkt[2]|((unsigned)pkt[3]<<8);
 size_t at=4;
 const unsigned char *fields[4]={NULL,NULL,NULL,NULL};
 size_t flens[4]={0,0,0,0};
 for(int f=0;f<4;f++){
  if(at>=plen)return 0;
  size_t fl=pkt[at++];
  if(at+fl>plen)return 0;
  fields[f]=pkt+at;flens[f]=fl;at+=fl;
 }
 media_elapsed_secs=(at+2<=plen)?((unsigned)pkt[at]|((unsigned)pkt[at+1]<<8)):0;
 size_t pos=0;
 if(pb_put_field_bytes(out,cap,&pos,1,fields[0],flens[0]))return 0; /* song/title */
 if(pb_put_field_bytes(out,cap,&pos,2,fields[1],flens[1]))return 0; /* artist */
 if(pb_put_field_bytes(out,cap,&pos,3,fields[2],flens[2]))return 0; /* album */
 if(duration&&pb_put_field_varint(out,cap,&pos,6,duration))return 0; /* duration_seconds */
 if(out_state)*out_state=state;
 if(app_out){
  size_t copy=flens[3]<63?flens[3]:63;
  if(copy){memcpy(app_out,fields[3],copy);app_out[copy]=0;}
  else strcpy(app_out,"CarPlay");
 }
 return pos;
}
static void media_metadata_on_control(const unsigned char *pkt,size_t plen){
 unsigned state=0;char app[64];
 size_t mlen=encode_media_metadata(media_meta_buf,sizeof media_meta_buf,pkt,plen,&state,app);
 if(!mlen)return;
 media_meta_len=mlen;
 snprintf(media_source_name,sizeof media_source_name,"%s",app);
 if(media_status_channel>0&&media_status_ready){
  send_msg(media_status_channel,11,0x8003,media_meta_buf,media_meta_len);
  if(state>=1&&state<=3)media_status_update(state);
  else if(audio_streaming)media_status_update(2);
  fprintf(stderr,"AA_MEDIA_META ch=%d bytes=%zu state=%u pos=%us src=%s\n",media_status_channel,media_meta_len,state,media_elapsed_secs,media_source_name);
  session_trace("AA_MEDIA_META ch=%d bytes=%zu state=%u pos=%us src=%s",media_status_channel,media_meta_len,state,media_elapsed_secs,media_source_name);
 }
}
/* Dedicated AA Guidance audio sink (e.g. Uconnect VP2 ch=5, 16 kHz mono PCM16) for Waze/Siri prompts. */
#define GUIDANCE_STOP_IDLE_US 1500000ULL
static int guidance_channel=-1,guidance_fd=-1,guidance_ready,guidance_streaming,guidance_unacked,guidance_max_unacked=1;
static int guidance_focus; /* 0=none, 1=requested MAY_DUCK, 2=granted GUIDANCE_ONLY */
static unsigned guidance_rate=16000,guidance_channels=1,guidance_session,guidance_sent,guidance_acks,guidance_underflows;
static uint64_t guidance_epoch,guidance_samples,guidance_last,guidance_focus_at,guidance_full_at;
static void guidance_stop(const char *why){
 if(!guidance_streaming)return;
 send_msg(guidance_channel,11,0x8002,"",0);guidance_streaming=0;guidance_unacked=0;
 session_trace("GUIDANCE_STOP session=%u %s sent=%u acks=%u",guidance_session,why,guidance_sent,guidance_acks);
}
static void guidance_start(uint64_t now,uint64_t start){
 unsigned char begin[8]={8};size_t n=1;unsigned id=++guidance_session;
 do{begin[n++]=(unsigned char)((id&127)|(id>=128?128:0));id>>=7;}while(id);begin[n++]=16;begin[n++]=0;
 send_msg(guidance_channel,11,0x8001,begin,n);guidance_streaming=1;guidance_unacked=0;guidance_epoch=now-start;guidance_samples=0;
 session_trace("GUIDANCE_START session=%u rate=%u chs=%u focus=%d/%d",guidance_session,guidance_rate,guidance_channels,audio_focus,guidance_focus);
}
static void guidance_bind_socket(void){
 const char *path=getenv("AA_VIDEO_SOCKET");
 if(!path||!*path||guidance_fd>=0)return;
 struct sockaddr_un addr={0};addr.sun_family=AF_UNIX;
 if(snprintf(addr.sun_path,sizeof addr.sun_path,"%s.guidance",path)>=(int)sizeof addr.sun_path)return;
 unlink(addr.sun_path);
 int s=socket(AF_UNIX,SOCK_DGRAM,0);
 if(s<0)return;
 int rbuf=65536;setsockopt(s,SOL_SOCKET,SO_RCVBUF,&rbuf,sizeof rbuf);
 if(bind(s,(struct sockaddr*)&addr,sizeof addr)){close(s);return;}
 fcntl(s,F_SETFL,O_NONBLOCK);
 guidance_fd=s;
 char fmt_path[4096];
 if(snprintf(fmt_path,sizeof fmt_path,"%s.guidance.fmt",path)<(int)sizeof fmt_path){
  FILE *f=fopen(fmt_path,"w");
  if(f){fprintf(f,"%u %u\n",guidance_rate,guidance_channels);fclose(f);}
 }
}
static void guidance_on_config(const unsigned char *body,size_t len){
 if(value(body,len,1,0)!=2){session_trace("GUIDANCE_CONFIG rejected %s",hex16(body,len));return;}
 guidance_max_unacked=audio_window(value(body,len,2,1));guidance_unacked=0;guidance_ready=1;
 guidance_bind_socket();
 fprintf(stderr,"GUIDANCE_READY ch=%d %u/%u window=%d\n",guidance_channel,guidance_rate,guidance_channels,guidance_max_unacked);
 session_trace("GUIDANCE_READY ch=%d rate=%u chs=%u window=%d",guidance_channel,guidance_rate,guidance_channels,guidance_max_unacked);
}
static void guidance_on_ack(const unsigned char *body,size_t len){
 int ack=(int)value(body,len,2,1);
 guidance_acks+=(unsigned)(ack>0?ack:0);
 guidance_unacked=ack>guidance_unacked?0:guidance_unacked-ack;
}
static void guidance_pump(uint64_t now,uint64_t start){
 if(!guidance_ready||guidance_fd<0)return;
 if(guidance_streaming&&guidance_unacked>=guidance_max_unacked&&now-guidance_full_at>2000000){
  session_trace("GUIDANCE_ACK_STALL unacked=%d window=%d",guidance_unacked,guidance_max_unacked);guidance_unacked=0;
 }
 size_t frame_bytes=(size_t)guidance_channels*2;
 for(unsigned batch=0;batch<8&&!(guidance_streaming&&guidance_unacked>=guidance_max_unacked);batch++){
  unsigned char data[8200];unsigned char *pcm=data+8;
  ssize_t count=recv(guidance_fd,pcm,sizeof data-8,MSG_DONTWAIT);
  if(count<=0)break;
  guidance_last=now;
  if(!guidance_streaming){
   if(audio_focus==2||guidance_focus==2)guidance_start(now,start);
   else{
    if(!guidance_focus_at||now-guidance_focus_at>1000000ULL){
     request_audio_focus(3,now); /* AUDIO_FOCUS_GAIN_TRANSIENT_MAY_DUCK */
     guidance_focus=1;guidance_focus_at=now;
    }
    continue;
   }
  }
  uint64_t ts=guidance_epoch+guidance_samples*1000000ULL/guidance_rate;
  uint64_t elapsed=now-start;
  if(elapsed>ts&&elapsed-ts>=AUDIO_MAX_LATE_US){ts=elapsed;guidance_epoch=ts;guidance_samples=0;}
  guidance_samples+=(uint64_t)count/frame_bytes;
  for(int k=7;k>=0;k--){data[k]=(unsigned char)ts;ts>>=8;}
  send_msg(guidance_channel,11,0,data,(size_t)count+8);guidance_sent++;
  if(++guidance_unacked>=guidance_max_unacked)guidance_full_at=now;
 }
 if(guidance_streaming&&now-guidance_last>GUIDANCE_STOP_IDLE_US){
  guidance_stop("idle");
  if(guidance_focus==2&&!audio_streaming&&audio_focus==0){
   request_audio_focus(4,now);
   guidance_focus=0;guidance_last=0;
   session_trace("GUIDANCE_FOCUS_RELEASED");
  }
 }
}
static int find_guidance_audio(const unsigned char *p,size_t n){
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return -1;if(t!=1||!b)continue;
  int id=(int)value(b,l,1,0);size_t j=0,sl;unsigned st;uint64_t sv;const unsigned char *sb;
  while(j<l){if(field(b,l,&j,&st,&sv,&sb,&sl))return -1;if(st!=3||!sb)continue;
   int codec=(int)value(sb,sl,1,1),type=(int)value(sb,sl,2,0);
   if(type!=1||codec!=1)continue;
   size_t k=0,al;unsigned at;uint64_t av;const unsigned char *ab;
   while(k<sl){if(field(sb,sl,&k,&at,&av,&ab,&al))return -1;if(at!=3||!ab)continue;
    unsigned rate=(unsigned)value(ab,al,1,0),bits=(unsigned)value(ab,al,2,0),channels=(unsigned)value(ab,al,3,0);
    if(id>0&&id<256&&bits==16&&(channels==1||channels==2)&&(rate==16000||rate==48000)){
     guidance_rate=rate;guidance_channels=channels;
     fprintf(stderr,"AA_GUIDANCE_SELECTED channel=%d %u/16/%s\n",id,rate,channels==1?"mono":"stereo");
     return id;
    }
   }
  }
 }
 return -1;
}
static void audio_start(uint64_t now,uint64_t start){
 unsigned char begin[8]={8};size_t n=1;unsigned id=++audio_session;
 do{begin[n++]=(unsigned char)((id&127)|(id>=128?128:0));id>>=7;}while(id);begin[n++]=16;begin[n++]=0;
 send_msg(audio_channel,11,0x8001,begin,n);audio_streaming=1;audio_unacked=0;audio_epoch=now-start;audio_samples=0;audio_prime_until=0;
 media_status_update(2);
 session_trace("AUDIO_START session=%u discarded=%u",audio_session,audio_discarded);
}
static void audio_stop(const char *why){
 if(!audio_streaming)return;
 send_msg(audio_channel,11,0x8002,"",0);audio_streaming=0;audio_unacked=0;
 media_status_update(3);
 session_trace("AUDIO_STOP session=%u %s sent=%u acks=%u",audio_session,why,audio_sent,audio_acks);
}
static int forward_input_report(const unsigned char *report,size_t len);
static void audio_on_focus_at(unsigned state,uint64_t now){
 if(state==7){
  /* AUDIO_FOCUS_STATE_GAIN_TRANSIENT_GUIDANCE_ONLY (Uconnect VP2 reply to MAY_DUCK). */
  audio_focus=0;guidance_focus=2;
  audio_stop("guidance_only");
  session_trace("AUDIO_FOCUS state=%u guidance_only",state);
  return;
 }
 audio_focus=state==1||state==2||state==4||state==6?2:0;
 if(audio_focus){
  guidance_focus=0;
  session_trace("AUDIO_FOCUS state=%u granted",state);
 }else{
  int was_streaming=audio_streaming,pause_sent=0;
  audio_stop("focus_lost");guidance_focus=0;guidance_stop("focus_lost");
  /* Hold off focus re-request while the iPhone flushes its RTP buffer after Pause. */
  audio_focus_at=now?now:1;
  if(was_streaming&&(state==3||state==5)){
   unsigned char pause_cmd[]={0x20,2};
   pause_sent=forward_input_report(pause_cmd,sizeof pause_cmd);
  }
  session_trace("AUDIO_FOCUS state=%u lost pause_sent=%d",state,pause_sent);
 }
}
static void audio_on_focus(unsigned state){audio_on_focus_at(state,audio_last?audio_last:now_us());}
static void audio_on_ack(const unsigned char *body,size_t len){
 int ack=(int)value(body,len,2,1);
 if(!audio_acks)session_trace("RX audio ack first %s unacked=%d",hex16(body,len),audio_unacked);
 audio_acks+=(unsigned)(ack>0?ack:0);audio_unacked=ack>audio_unacked?0:audio_unacked-ack;
}
static void audio_pump(uint64_t now,uint64_t start){
 if(!audio_ready||audio_fd<0)return;
 if(audio_streaming&&audio_pump_at&&now>=audio_pump_at){
  uint64_t gap=now-audio_pump_at;
  if(gap>audio_pump_gap_max)audio_pump_gap_max=gap;
 }
 audio_pump_at=now;
 if(!audio_streaming&&audio_focus==2&&audio_preroll_us){
  unsigned char first_byte;
  if(!audio_prime_until&&recv(audio_fd,&first_byte,1,MSG_PEEK|MSG_DONTWAIT)>0)
   audio_prime_until=now+audio_preroll_us;
  if(audio_prime_until>now)return;
 }else audio_prime_until=0;
 /* A head unit that stops acknowledging must not mute the stream for good. */
 if(audio_streaming&&audio_unacked>=audio_max_unacked&&now-audio_full_at>2000000){audio_stalls++;session_trace("AUDIO_ACK_STALL unacked=%d window=%d acks=%u",audio_unacked,audio_max_unacked,audio_acks);audio_unacked=0;}
 /* Yield to USB control, microphone and video even when PCM is backlogged. */
 for(unsigned batch=0;batch<8&&!(audio_streaming&&audio_unacked>=audio_max_unacked);batch++){
  unsigned char data[8200];unsigned char *pcm=data+8;
  ssize_t count=recv(audio_fd,pcm,sizeof data-8,MSG_DONTWAIT);
  if(count<=0)break;
  if(audio_streaming&&audio_last&&now>=audio_last&&now-audio_last>=550000){
   session_trace("AUDIO_PCM_GAP gap_us=%llu max_loop_gap_us=%llu bytes=%zd",
    (unsigned long long)(now-audio_last),(unsigned long long)audio_pump_gap_max,count);
  }
  audio_pump_gap_max=0;
  if(!audio_last){session_trace("AUDIO_PCM_START bytes=%zd focus=%d window=%d",count,audio_focus,audio_max_unacked);audio_stats_at=now;}
  audio_last=now;
  for(ssize_t k=0;k+1<count;k+=2){int sample=(int16_t)(pcm[k]|pcm[k+1]<<8);unsigned level=(unsigned)(sample<0?-sample:sample);if(level>audio_peak)audio_peak=level;}
  if(!audio_streaming){
   if(audio_focus==2)audio_start(now,start);
   else{if(!audio_focus_at||now-audio_focus_at>AUDIO_FOCUS_RETRY_US){request_audio_focus(1,now);audio_focus=1;}audio_discarded++;continue;}
  }
  uint64_t ts=audio_epoch+audio_samples*1000000ULL/48000;
  uint64_t elapsed=now-start;
  if(elapsed>ts&&elapsed-ts>=AUDIO_MAX_LATE_US){
   session_trace("AUDIO_CLOCK_RESYNC late_us=%llu",(unsigned long long)(elapsed-ts));
   ts=elapsed;audio_epoch=ts;audio_samples=0;
  }
  audio_samples+=(uint64_t)count/4;
  for(int k=7;k>=0;k--){data[k]=(unsigned char)ts;ts>>=8;}
  send_msg(audio_channel,11,0,data,(size_t)count+8);audio_sent++;
  if(++audio_unacked>=audio_max_unacked)audio_full_at=now;
  TRACE("AUDIO sent bytes=%zd\n",count);
 }
 /* Delivery counters and PCM peak level: the car log shows whether audio reached the sink. */
 if(audio_last&&now-audio_stats_at>=5000000){audio_stats_at=now;session_trace("AUDIO_STATS sent=%u acks=%u unacked=%d window=%d session=%u streaming=%d discarded=%u underflows=%u peak=%u focus=%d",audio_sent,audio_acks,audio_unacked,audio_max_unacked,audio_session,audio_streaming,audio_discarded,audio_underflows,audio_peak,audio_focus);audio_peak=0;}
 if(audio_streaming&&now-audio_last>AUDIO_STOP_IDLE_US)audio_stop("idle");
 if(audio_last&&now-audio_last>AUDIO_RELEASE_IDLE_US){if(audio_focus)request_audio_focus(4,now);audio_focus=0;audio_last=0;session_trace("AUDIO_PCM_IDLE sent=%u acks=%u underflows=%u",audio_sent,audio_acks,audio_underflows);}
}

/* Head-unit PCM source -> local CarPlay recorder. Only mono PCM16 is accepted. */
static int mic_channel=-1,mic_ready,mic_wanted,mic_open,mic_session=1;
static unsigned mic_rate;static unsigned long mic_frames;
static int find_microphone(const unsigned char *p,size_t n){
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return -1;if(t!=1||!b)continue;
  int id=(int)value(b,l,1,0);size_t j=0,sl;unsigned st;uint64_t sv;const unsigned char *sb;
  while(j<l){if(field(b,l,&j,&st,&sv,&sb,&sl))return -1;if(st!=5||!sb)continue;
   unsigned codec=(unsigned)value(sb,sl,1,0);
   size_t k=0,al;unsigned at;uint64_t av;const unsigned char *ab;
   while(k<sl){if(field(sb,sl,&k,&at,&av,&ab,&al))return -1;if(at!=2||!ab)continue;
    unsigned rate=value(ab,al,1,0),bits=value(ab,al,2,0),channels=value(ab,al,3,0);
    fprintf(stderr,"AA_MIC_OFFER channel=%d codec=%u rate=%u bits=%u channels=%u\n",id,codec,rate,bits,channels);
    session_trace("AA_MIC_OFFER ch=%d codec=%u rate=%u bits=%u chs=%u",id,codec,rate,bits,channels);
    if(codec==1&&id>0&&id<256&&bits==16&&channels==1&&rate>=8000&&rate<=48000){mic_rate=rate;fprintf(stderr,"AA_MIC_SELECTED channel=%d rate=%u PCM16 mono\n",id,rate);session_trace("AA_MIC_SELECTED ch=%d rate=%u",id,rate);return id;}
   }
  }
 }fprintf(stderr,"AA_MIC_UNAVAILABLE: no PCM16 mono source\n");session_trace("AA_MIC_UNAVAILABLE");return -1;
}
static void microphone_request(void){
 if(!mic_ready||mic_channel<0||mic_open==mic_wanted)return;
 /* Uconnect VP2 (CVTOPROJECTIONDEVICE.DLL): 0x8005 ConfigureMicrophoneAA is a stub; hardware mic
  * capture (PerformAndroidVRInit) starts on ch=0 VoiceSessionNotification (0x11) status=1 (START)
  * once AA media playback is stopped, and stops on status=2 (END). Send both 0x11 and 0x8005. */
 unsigned char request[]={8,(unsigned char)mic_wanted,16,1,24,1,32,4};
 if(mic_wanted){
  if(audio_streaming)audio_stop("voice");
  unsigned char vs[]={0x08,1};
  send_msg(0,11,0x11,vs,sizeof vs);
  session_trace("AA_VOICE_SESSION state=1");
  send_msg(mic_channel,11,0x8005,request,sizeof request);
 }else{
  send_msg(mic_channel,11,0x8005,request,sizeof request);
  unsigned char vs[]={0x08,2};
  send_msg(0,11,0x11,vs,sizeof vs);
  session_trace("AA_VOICE_SESSION state=2");
 }
 mic_open=mic_wanted;fprintf(stderr,"AA_MIC_REQUEST open=%d\n",mic_open);session_trace("AA_MIC_REQUEST ch=%d open=%d",mic_channel,mic_open);
}
static int microphone_on_open_response(const unsigned char *body,size_t len){
 int32_t f1=(int32_t)(uint32_t)value(body,len,1,0),f2=(int32_t)(uint32_t)value(body,len,2,0);
 if(f1<0||f2<0){
  mic_open=0;fprintf(stderr,"AA_MIC open rejected f1=%d f2=%d\n",f1,f2);
  session_trace("AA_MIC_OPEN_REJECTED f1=%d f2=%d %s",f1,f2,hex16(body,len));
  return -1;
 }
 mic_session=f1>0?(int)f1:(int)f2;
 fprintf(stderr,"AA_MIC_OPENED session=%d open=%d\n",mic_session,mic_open);
 session_trace("AA_MIC_OPENED session=%d open=%d f1=%d f2=%d %s",mic_session,mic_open,f1,f2,hex16(body,len));
 return 0;
}
static void microphone_pcm(const unsigned char *body,size_t len,int timestamp){
 size_t skip=timestamp?8:0;if(!mic_open||len<=skip||(len-skip)%2)return;
 const char *path=getenv("AA_VIDEO_SOCKET");if(!path)return;
 struct sockaddr_un target={0};target.sun_family=AF_UNIX;
 if(snprintf(target.sun_path,sizeof target.sun_path,"%s.mic",path)>=(int)sizeof target.sun_path)return;
 int sock=socket(AF_UNIX,SOCK_DGRAM,0);if(sock<0)return;
 unsigned char packet[8196];packet[0]=mic_rate;packet[1]=mic_rate>>8;packet[2]=mic_rate>>16;packet[3]=mic_rate>>24;
 for(size_t at=skip;at<len;){size_t count=len-at;if(count>8192)count=8192;memcpy(packet+4,body+at,count);
  if(sendto(sock,packet,count+4,MSG_DONTWAIT,(struct sockaddr*)&target,sizeof target)<0){if(mic_frames%100==0){perror("AA_MIC_FORWARD");session_trace("AA_MIC_FORWARD_ERR errno=%d",errno);}break;}at+=count;
 }close(sock);mic_frames++;if(mic_frames%100==1){fprintf(stderr,"AA_MIC_PCM frames=%lu rate=%u\n",mic_frames,mic_rate);session_trace("AA_MIC_PCM frames=%lu rate=%u bytes=%zu",mic_frames,mic_rate,len-skip);}
}
static int find_media_audio(const unsigned char *p,size_t n){
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return -1;if(t!=1||!b)continue;
  int id=(int)value(b,l,1,0);size_t j=0,sl;unsigned st;uint64_t sv;const unsigned char *sb;
  while(j<l){if(field(b,l,&j,&st,&sv,&sb,&sl))return -1;if(st!=3||!sb)continue;
   int codec=(int)value(sb,sl,1,1),type=(int)value(sb,sl,2,0);size_t k=0,al;unsigned at;uint64_t av;const unsigned char *ab;int index=0;
   while(k<sl){if(field(sb,sl,&k,&at,&av,&ab,&al))return -1;if(at!=3||!ab)continue;
    unsigned rate=value(ab,al,1,0),bits=value(ab,al,2,0),channels=value(ab,al,3,0);
    fprintf(stderr,"AA_AUDIO_OFFER channel=%d type=%d codec=%d config=%d rate=%u bits=%u channels=%u\n",id,type,codec,index,rate,bits,channels);
    if(type==3&&codec==1&&index==0&&rate==48000&&bits==16&&channels==2){fprintf(stderr,"AA_AUDIO_SELECTED channel=%d 48000/16/stereo\n",id);return id;}
    index++;
   }
  }
 }fprintf(stderr,"AA_AUDIO_DISABLED: no compatible primary media PCM 48000/16/stereo configuration\n");return -1;
}
static int forward_input_report(const unsigned char *report,size_t len){
 const char *path=getenv("AA_VIDEO_SOCKET");
 if(!path)return 0;
 struct sockaddr_un target={.sun_family=AF_UNIX};
 if(snprintf(target.sun_path,sizeof target.sun_path,"%s.touch",path)>=(int)sizeof target.sun_path)return 0;
 int sock=socket(AF_UNIX,SOCK_DGRAM,0);
 if(sock<0)return 0;
 ssize_t sent=sendto(sock,report,len,MSG_DONTWAIT,(struct sockaddr*)&target,sizeof target);
 close(sock);
 return sent==(ssize_t)len;
}

/* Android Auto key binding -> CarPlay input socket report. */
static const struct {
 unsigned code;
 unsigned char type, button;
} input_bindings[]={
 {0x01,0x22,1}, {0x02,0x23,2}, {0x03,0x23,2}, {0x04,0x23,4},
 {0x05,0x21,1}, {0x06,0x21,3},
 {0x13,0x24,1}, {0x14,0x24,2}, {0x15,0x24,3}, {0x16,0x24,4}, {0x17,0x23,1},
 {0x18,0x20,7}, {0x19,0x20,8},
 {0x42,0x23,1}, {0x4f,0x21,1},
 {0x54,0x22,1}, {0x55,0x20,3}, {0x56,0x20,10}, {0x57,0x20,4}, {0x58,0x20,5},
 {0x59,0x20,11}, {0x5a,0x20,12}, {0x5b,0x21,4},
 {0x7e,0x20,1}, {0x7f,0x20,2}, {0xa4,0x20,9}, {0xdb,0x22,1}, {0xe7,0x22,1},
 {0x10004,0x21,1},
};
enum { AA_SCROLL_WHEEL=65536 };
enum { INPUT_BINDING_COUNT=sizeof input_bindings/sizeof *input_bindings };
_Static_assert(INPUT_BINDING_COUNT<64,"input binding mask too small");

static uint64_t input_key_mask(uint64_t code){
 if(code==AA_SCROLL_WHEEL)return UINT64_C(1)<<INPUT_BINDING_COUNT;
 for(size_t i=0;i<INPUT_BINDING_COUNT;i++){
  if(input_bindings[i].code==code)return UINT64_C(1)<<i;
 }
 return 0;
}

static uint64_t offered_input_keys(const unsigned char *data,size_t size){
 uint64_t mask=0,code;
 size_t offset=0,len;
 unsigned tag,count=0;
 const unsigned char *packed;
 char listed[256];size_t listed_len=0;listed[0]=0;
 while(offset<size){
  if(field(data,size,&offset,&tag,&code,&packed,&len))return 0;
  if(tag!=1)continue;
  if(!packed){
   count++;mask|=input_key_mask(code);
   if(listed_len+16<sizeof listed)listed_len+=(size_t)snprintf(listed+listed_len,sizeof listed-listed_len,"%s0x%llx",listed_len?",":"",(unsigned long long)code);
   continue;
  }
  for(size_t item=0;item<len;){
   if(varint(packed,len,&item,&code))return 0;
   count++;mask|=input_key_mask(code);
   if(listed_len+16<sizeof listed)listed_len+=(size_t)snprintf(listed+listed_len,sizeof listed-listed_len,"%s0x%llx",listed_len?",":"",(unsigned long long)code);
  }
 }
 session_trace("AA_INPUT_OFFER count=%u mask=0x%llx codes=%s",count,(unsigned long long)mask,listed[0]?listed:"none");
 return mask;
}

static int find_input(const unsigned char *p,size_t n,uint64_t *keys){
 int selected=-1;
 *keys=0;
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){
  if(field(p,n,&i,&t,&v,&b,&l))return -1;
  if(t!=1||!b)continue;
  int id=-1;
  size_t j=0,sl,input_len=0;unsigned st;uint64_t sv;const unsigned char *sb,*input=NULL;
  while(j<l){
   if(field(b,l,&j,&st,&sv,&sb,&sl))return -1;
   if(st==1&&!sb)id=(int)sv;
   if(st==4&&sb){input=sb;input_len=sl;}
  }
  if(input&&id>0&&id<256&&(id==8||selected<0)){
   selected=id;
   *keys=offered_input_keys(input,input_len);
   if(id==8)break;
  }
 }
 return selected;
}

static size_t encode_input_bindings(unsigned char *bindings,uint64_t offered){
 /* Android Auto BindingRequest (0x8002) is proto2 repeated int32 scan_codes = 1 (unpacked).
  * Unpacked 0x08 <varint> is accepted by both proto2 head units (Uconnect VP2) and proto3 (DHU). */
 size_t length=0;
 for(size_t i=0;i<=INPUT_BINDING_COUNT;i++){
  if(!(offered&(UINT64_C(1)<<i)))continue;
  unsigned code=i==INPUT_BINDING_COUNT?AA_SCROLL_WHEEL:input_bindings[i].code;
  bindings[length++]=0x08;
  while(code>=128){bindings[length++]=(code&127)|128;code>>=7;}
  bindings[length++]=(unsigned char)code;
 }
 return length;
}

static void forward_car_controls(const unsigned char *p,size_t n){
 size_t offset=0,length;
 unsigned tag;
 uint64_t number;
 const unsigned char *events;
 while(offset<n){
  if(field(p,n,&offset,&tag,&number,&events,&length))return;
  if((tag!=4&&tag!=5&&tag!=6)||!events)continue;
  unsigned event_type=tag;
  size_t cursor=0,event_size;
  const unsigned char *event;
  while(cursor<length){
   if(field(events,length,&cursor,&tag,&number,&event,&event_size))return;
   if(tag!=1||!event)continue;
   unsigned code=(unsigned)value(event,event_size,1,0);
   uint64_t state=value(event,event_size,2,0);
   if(event_type!=4){
    int32_t delta=(int32_t)(uint32_t)state;
    if(event_type==6&&code==AA_SCROLL_WHEEL){
     int bounded=delta<-127?-127:delta>127?127:delta;
     if(bounded){
      unsigned char report[]={0x25,(unsigned char)bounded};
      int forwarded=forward_input_report(report,sizeof report);
      fprintf(stderr,"AA_AXIS type=%u code=%u value=%d mapped=1 forwarded=%d\n",event_type,code,delta,forwarded);
      session_trace("AA_AXIS type=%u code=%u value=%d mapped=1 forwarded=%d",event_type,code,delta,forwarded);
     }
    }else{
     fprintf(stderr,"AA_AXIS type=%u code=%u value=%d mapped=0\n",event_type,code,delta);
     session_trace("AA_AXIS type=%u code=%u value=%d mapped=0",event_type,code,delta);
    }
    continue;
   }
   size_t i=0;
   for(;i<sizeof input_bindings/sizeof *input_bindings;i++)if(input_bindings[i].code==code)break;
   int mapped=i<sizeof input_bindings/sizeof *input_bindings;
   int long_press=value(event,event_size,4,0)!=0;
   int forwarded=0;
   if(mapped){
    /* Uconnect VP2 reports VR long-press (0x54, down=1, long=1) without a matching release; signal tap (2). */
    unsigned char val=state?(input_bindings[i].type==0x22&&long_press?2:input_bindings[i].button):0;
    unsigned char report[]={input_bindings[i].type,val};
    forwarded=forward_input_report(report,sizeof report);
   }
   if(state||!mapped){
    fprintf(stderr,"AA_BUTTON code=0x%x pressed=%d long=%d mapped=%d forwarded=%d\n",code,state!=0,long_press,mapped,forwarded);
    session_trace("AA_BUTTON code=0x%x pressed=%d long=%d mapped=%d forwarded=%d",code,state!=0,long_press,mapped,forwarded);
   }
  }
 }
}
static void update_night_mode(int night){
 night=night?1:0;
 if(car_night_mode==night)return;
 car_night_mode=night;
 const char *sock=getenv("AA_VIDEO_SOCKET");
 if(sock&&*sock){
  char path[4096];
  if(snprintf(path,sizeof path,"%s.night",sock)<(int)sizeof path){
   FILE *f=fopen(path,"w");
   if(f){fprintf(f,"%d\n",night);fclose(f);}
  }
 }
 unsigned char report[]={0x26,(unsigned char)night};
 int forwarded=forward_input_report(report,sizeof report);
 fprintf(stderr,"AA_NIGHT_MODE night=%d forwarded=%d\n",night,forwarded);
 session_trace("AA_NIGHT_MODE night=%d forwarded=%d",night,forwarded);
}
static uint64_t sensor_gps_file_at;
static int32_t sensor_last_speed_e3=-1;
static void write_gps_status(int32_t lat_e7,int32_t lon_e7,int32_t speed_e3,uint64_t now){
 if(sensor_gps_file_at&&now-sensor_gps_file_at<2000000ULL)return;
 sensor_gps_file_at=now;
 const char *sock=getenv("AA_VIDEO_SOCKET");
 if(!sock||!*sock)return;
 char path[4096];
 if(snprintf(path,sizeof path,"%s.gps",sock)>=(int)sizeof path)return;
 FILE *f=fopen(path,"w");
 if(!f)return;
 int32_t spd=speed_e3>=0?speed_e3:sensor_last_speed_e3;
 if(spd>=0)fprintf(f,"%.5f,%.5f %.0f km/h\n",lat_e7/1e7,lon_e7/1e7,spd*0.0036);
 else fprintf(f,"%.5f,%.5f\n",lat_e7/1e7,lon_e7/1e7);
 fclose(f);
}
static void parse_sensor_batch(const unsigned char *p,size_t n){
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){
  if(field(p,n,&i,&t,&v,&b,&l))return;
  if(!b)continue;
  if(t==1){
   /* LocationData: 2=lat_e7, 3=lon_e7, 4=accuracy_e3, 5=altitude_e2, 6=speed_e3, 7=bearing_e6 */
   int has_lat=0,has_lon=0;
   int32_t lat_e7=0,lon_e7=0,alt_e2=INT32_MIN,speed_e3=-1,bearing_e6=-1;
   uint32_t acc_e3=0,flags=0;
   size_t j=0,fl;unsigned ft;uint64_t fv;const unsigned char *fb;
   while(j<l){
    if(field(b,l,&j,&ft,&fv,&fb,&fl))break;
    if(fb)continue;
    switch(ft){
     case 2: lat_e7=(int32_t)(uint32_t)fv;has_lat=1;break;
     case 3: lon_e7=(int32_t)(uint32_t)fv;has_lon=1;break;
     case 4: acc_e3=(uint32_t)fv;flags|=1u;break;
     case 5: alt_e2=(int32_t)(uint32_t)fv;flags|=2u;break;
     case 6: speed_e3=(int32_t)(uint32_t)fv;flags|=4u;break;
     case 7: bearing_e6=(int32_t)(uint32_t)fv;flags|=8u;break;
     default: break;
    }
   }
   if(has_lat&&has_lon&&(lat_e7!=0||lon_e7!=0)&&
      lat_e7>=-900000000&&lat_e7<=900000000&&lon_e7>=-1800000000&&lon_e7<=1800000000){
    unsigned char report[29];
    report[0]=0x27;
    put_le32(report+1,(uint32_t)lat_e7);
    put_le32(report+5,(uint32_t)lon_e7);
    put_le32(report+9,acc_e3);
    put_le32(report+13,(uint32_t)alt_e2);
    put_le32(report+17,(uint32_t)speed_e3);
    put_le32(report+21,(uint32_t)bearing_e6);
    put_le32(report+25,flags);
    int forwarded=forward_input_report(report,sizeof report);
    write_gps_status(lat_e7,lon_e7,(flags&4u)?speed_e3:-1,now_us());
    if(sensor_gps_count++%60==0){
     session_trace("AA_GPS_FIX n=%u lat=%d lon=%d alt=%d spd=%d brg=%d fwd=%d",
      sensor_gps_count,lat_e7,lon_e7,(flags&2u)?alt_e2:0,(flags&4u)?speed_e3:-1,(flags&8u)?bearing_e6:-1,forwarded);
    }
   }
  }else if(t==3){
   /* SpeedData: 1=speed_e3 (m/s * 1000) */
   size_t j=0,fl;unsigned ft;uint64_t fv;const unsigned char *fb;
   int has_speed=0;int32_t speed_e3=0;
   while(j<l){
    if(field(b,l,&j,&ft,&fv,&fb,&fl))break;
    if(!fb&&ft==1){speed_e3=(int32_t)(uint32_t)fv;has_speed=1;}
   }
   if(has_speed){
    sensor_last_speed_e3=speed_e3;
    unsigned char report[6]={0x28,0,0,0,0,0};
    put_le32(report+1,(uint32_t)speed_e3);
    int forwarded=forward_input_report(report,sizeof report);
    if(sensor_speed_count++%120==0){
     session_trace("AA_SPEED n=%u speed_e3=%d fwd=%d",sensor_speed_count,speed_e3,forwarded);
    }
   }
  }else if(t==10){
   /* NightModeData: 1=is_night */
   update_night_mode(value(b,l,1,0)!=0);
  }
 }
}
enum dashboard_surface { DASHBOARD_NONE, DASHBOARD_MENU, DASHBOARD_WAITING };
static enum dashboard_surface dashboard_visible;
static void dashboard_touch(unsigned action,unsigned x,unsigned y){
 const char *dir=getenv("AA_DASHBOARD_DIR");
 if(!dir)return;
 struct sockaddr_un target={.sun_family=AF_UNIX};
 if(snprintf(target.sun_path,sizeof target.sun_path,"%s/input.sock",dir)>=(int)sizeof target.sun_path)return;
 int sock=socket(AF_UNIX,SOCK_DGRAM|SOCK_CLOEXEC,0);
 if(sock<0)return;
 unsigned char report[5]={action,x&255,x>>8,y&255,y>>8};
 sendto(sock,report,sizeof report,MSG_DONTWAIT,(struct sockaddr*)&target,sizeof target);
 close(sock);
}
static void forward_touch(const unsigned char *p,size_t n){
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return;if(t!=3||!b)continue;
  uint64_t action=value(b,l,3,99);if(action>2)return;
  size_t j=0,pl;unsigned pt;uint64_t pv;const unsigned char *pointer;
  while(j<l){if(field(b,l,&j,&pt,&pv,&pointer,&pl))return;if(pt!=1||!pointer)continue;
   unsigned x=(unsigned)value(pointer,pl,1,99999),y=(unsigned)value(pointer,pl,2,99999);
   if(x>=display_width||y>=display_height)return;
   unsigned char report[5]={action!=1,x&255,x>>8,y&255,y>>8};
   if(dashboard_visible)dashboard_touch((unsigned)action|(dashboard_visible==DASHBOARD_WAITING?0x80:0),x,y);
   else forward_input_report(report,sizeof report);
   return;
  }
 }
}
static void open_video(int ch){unsigned char b[]={8,0,16,(unsigned char)ch,0};size_t n=4;if(ch>=128){b[3]=(unsigned char)((ch&127)|128);b[4]=(unsigned char)(ch>>7);n=5;}send_msg(ch,15,7,b,n);}
#define LIVE_MAX (1024*1024)
static int live_listener=-1,live_peer=-1,control_fd=-1;
static unsigned char live_buf[LIVE_MAX+12],video_message[LIVE_MAX+10];
static size_t live_used;
static int live_disconnected;
static int live_lock=-1;
static void reclaim_live_sockets(const char *path){
 char name[sizeof(((struct sockaddr_un *)0)->sun_path)];
 const char *suffixes[]={"",".audio",".control",".guidance"};
 if(snprintf(name,sizeof name,"%s.lock",path)>=(int)sizeof name){errno=ENAMETOOLONG;die("Live lock path");}
 live_lock=open(name,O_CREAT|O_RDWR|O_CLOEXEC|O_NOFOLLOW,0600);
 if(live_lock<0||flock(live_lock,LOCK_EX|LOCK_NB))die("Live sender already running or lock unavailable");
 for(unsigned i=0;i<4;i++){
  struct stat st;
  if(snprintf(name,sizeof name,"%s%s",path,suffixes[i])>=(int)sizeof name){errno=ENAMETOOLONG;die("Live socket path");}
  if(lstat(name,&st)==0){
   if(!S_ISSOCK(st.st_mode)){errno=EEXIST;die("Refusing to replace a non-socket live path");}
   if(unlink(name))die("Reclaim stale live socket");
  }else if(errno!=ENOENT)die("Inspect live socket");
 }
}
static void open_live_input(const char *path){
 reclaim_live_sockets(path);
 struct sockaddr_un a={0};a.sun_family=AF_UNIX;
 if(strlen(path)>=sizeof a.sun_path){fprintf(stderr,"Video socket path too long\n");exit(1);}
 strcpy(a.sun_path,path);live_listener=socket(AF_UNIX,SOCK_STREAM,0);if(live_listener<0)die("Video socket");
 int vbuf=524288;setsockopt(live_listener,SOL_SOCKET,SO_RCVBUF,&vbuf,sizeof vbuf);
 if(bind(live_listener,(struct sockaddr*)&a,sizeof a)||listen(live_listener,1))die("Video bind/listen");
 fcntl(live_listener,F_SETFL,O_NONBLOCK);
 struct sockaddr_un audio={0};audio.sun_family=AF_UNIX;
 if(snprintf(audio.sun_path,sizeof audio.sun_path,"%s.audio",path)>=(int)sizeof audio.sun_path)exit(1);
 audio_fd=socket(AF_UNIX,SOCK_DGRAM,0);if(audio_fd<0||bind(audio_fd,(struct sockaddr*)&audio,sizeof audio))die("Audio socket");
 int abuf=262144;setsockopt(audio_fd,SOL_SOCKET,SO_RCVBUF,&abuf,sizeof abuf);
 fcntl(audio_fd,F_SETFL,O_NONBLOCK);
 struct sockaddr_un control={0};control.sun_family=AF_UNIX;
 if(snprintf(control.sun_path,sizeof control.sun_path,"%s.control",path)>=(int)sizeof control.sun_path)exit(1);
 control_fd=socket(AF_UNIX,SOCK_DGRAM,0);if(control_fd<0||bind(control_fd,(struct sockaddr*)&control,sizeof control))die("Control socket");
 fcntl(control_fd,F_SETFL,O_NONBLOCK);
 {char aux_path[sizeof control.sun_path];
  if(snprintf(aux_path,sizeof aux_path,"%s.night",path)<(int)sizeof aux_path)unlink(aux_path);
  if(snprintf(aux_path,sizeof aux_path,"%s.btmac",path)<(int)sizeof aux_path)unlink(aux_path);
  if(snprintf(aux_path,sizeof aux_path,"%s.gps",path)<(int)sizeof aux_path)unlink(aux_path);
  if(snprintf(aux_path,sizeof aux_path,"%s.guidance.fmt",path)<(int)sizeof aux_path)unlink(aux_path);
 }
 fprintf(stderr,"LIVE_VIDEO_READY %s\n",path);
}
static void poll_live_disconnect(void){
  if(live_peer>=0){unsigned char peek;ssize_t n=recv(live_peer,&peek,1,MSG_PEEK|MSG_DONTWAIT);
   if(n==0||(n<0&&errno!=EAGAIN&&errno!=EWOULDBLOCK&&errno!=EINTR)){close(live_peer);live_peer=-1;live_used=0;live_disconnected=1;}
  }
}
static int next_live_frame(size_t *length,uint64_t *stamp){
 if(live_peer<0){live_peer=accept(live_listener,NULL,NULL);if(live_peer<0)return 0;fcntl(live_peer,F_SETFL,O_NONBLOCK);live_used=0;
  if(car_night_mode>=0){unsigned char report[]={0x26,(unsigned char)car_night_mode};forward_input_report(report,sizeof report);}
 }
 size_t needed=12;
 for(;;){
  if(live_used>=12){needed=12+((size_t)live_buf[0]<<24| (size_t)live_buf[1]<<16| (size_t)live_buf[2]<<8|live_buf[3]);
   if(needed<=12||needed>sizeof live_buf){fprintf(stderr,"Invalid live frame size\n");exit(1);}
   if(live_used==needed){*length=needed-12;*stamp=0;for(int k=4;k<12;k++)*stamp=(*stamp<<8)|live_buf[k];return 1;}
  }
  ssize_t n=read(live_peer,live_buf+live_used,needed-live_used);
  if(n<0&&(errno==EAGAIN||errno==EWOULDBLOCK))return 0;
  if(n<0&&errno==EINTR)continue;
  if(n<=0){close(live_peer);live_peer=-1;live_used=0;live_disconnected=1;return 0;}
  live_used+=(size_t)n;
 }
}
/* Uconnect VP2 drops every H.264 sample until MEDIA_CODEC_CONFIG (id 1) supplies SPS/PPS,
 * as Android phones send it; parameter sets therefore leave the timestamped data message,
 * like SEI and AUD units, which phones do not send either. */
static unsigned char codec_config[1024],config_scratch[1024];
static size_t codec_config_size;
static int codec_config_pending=1,frames_since_config;
static size_t annexb_next(const unsigned char *p,size_t n,size_t i){
 while(i+3<=n){
  const unsigned char *z=memchr(p+i,0,n-i-2);
  if(!z)return n;
  i=(size_t)(z-p);
  if(!p[i+1]&&(p[i+2]==1||(i+4<=n&&!p[i+2]&&p[i+3]==1)))return i;
  i++;
 }
 return n;
}
static size_t split_parameter_sets(const unsigned char *in,size_t n,unsigned char *out,size_t cap,size_t *config_size){
 size_t at=annexb_next(in,n,0),used=at;memcpy(out,in,at);*config_size=0;
 while(at<n){size_t code=in[at+2]==1?3:4,end=annexb_next(in,n,at+code);unsigned type=at+code<n?in[at+code]&31:0;
  if((type==7||type==8)&&*config_size+end-at<=sizeof config_scratch){
   size_t k=type==7&&h264_poc2?poc_rewrite_sps(in+at+code,end-at-code,config_scratch+*config_size+code,sizeof config_scratch-*config_size-code,0):0;
   if(type==8)poc_parse_pps(in+at+code,end-at-code);
   if(k){memcpy(config_scratch+*config_size,in+at,code);*config_size+=code+k;}
   else{memcpy(config_scratch+*config_size,in+at,end-at);*config_size+=end-at;}
  }
  else if(type!=6&&type!=9){
   size_t k=(type==1||type==5)&&used+code<cap?poc_rewrite_slice(in+at+code,end-at-code,out+used+code,cap-used-code):0;
   if(k){memcpy(out+used,in+at,code);used+=code+k;}
   else{memcpy(out+used,in+at,end-at);used+=end-at;}
  }
  at=end;
 }
 return used;
}
/* Consecutive IDR pictures must carry different idr_pic_id: alternate the first two coded pictures. */
static size_t waiting_unit_off[2],waiting_unit_len[2];
static int waiting_units,waiting_next;
static int index_idr_units(const unsigned char *video,size_t length,size_t offsets[2],size_t lengths[2]){
 int units=0;
 size_t at=annexb_next(video,length,0),begin=0;
 while(at<length&&units<2){size_t code=video[at+2]==1?3:4,end=annexb_next(video,length,at+code);
  unsigned type=at+code<length?video[at+code]&31:0;
  if(type>=1&&type<=5){offsets[units]=begin;lengths[units++]=end-begin;begin=end;}
  at=end;
 }
 return units;
}
static void index_waiting_units(void){
 waiting_units=index_idr_units(waiting_video,waiting_size,waiting_unit_off,waiting_unit_len);
}

struct dashboard_picture {
 unsigned char *video;
 struct stat stamp;
 size_t offsets[2],lengths[2];
 int units,next,changed;
 uint64_t last_sent;
};
static struct dashboard_picture menu_picture,wait_picture;

enum projection_screen { PROJECTION_MENU, PROJECTION_WAITING, PROJECTION_CARPLAY };

static enum projection_screen initial_projection_screen(int dashboard_enabled){
 return dashboard_enabled?PROJECTION_MENU:PROJECTION_WAITING;
}

static enum projection_screen projection_after_disconnect(enum projection_screen screen,int dashboard_enabled){
 return screen==PROJECTION_CARPLAY?initial_projection_screen(dashboard_enabled):screen;
}

static int waiting_picture_due(enum projection_screen screen,uint64_t now,uint64_t next){
 return screen==PROJECTION_WAITING&&now>=next&&waiting_units&&(waiting_units>1||codec_config_pending);
}

static int dashboard_refresh(struct dashboard_picture *picture,const char *name){
 const char *dir=getenv("AA_DASHBOARD_DIR");
 if(!dir)return 0;
 char path[4096];
 if(snprintf(path,sizeof path,"%s/%s-%ux%u.h264",dir,name,display_width,display_height)>=(int)sizeof path)return 0;
 struct stat st;
 if(stat(path,&st)||st.st_size<=0||st.st_size>LIVE_MAX)return 0;
 if(picture->units&&st.st_ino==picture->stamp.st_ino&&st.st_mtim.tv_sec==picture->stamp.st_mtim.tv_sec&&
    st.st_mtim.tv_nsec==picture->stamp.st_mtim.tv_nsec&&st.st_size==picture->stamp.st_size)return 1;
 FILE *file=fopen(path,"rb");
 if(!file)return picture->units!=0;
 if(fstat(fileno(file),&st)||st.st_size<=0||st.st_size>LIVE_MAX){fclose(file);return picture->units!=0;}
 size_t len=(size_t)st.st_size;
 unsigned char *image=malloc(len);
 if(!image){fclose(file);return picture->units!=0;}
 size_t actual=fread(image,1,len,file);
 fclose(file);
 size_t offsets[2],lengths[2];
 int units=actual==len?index_idr_units(image,len,offsets,lengths):0;
 if(!units){free(image);return picture->units!=0;}
 free(picture->video);
 picture->video=image;picture->stamp=st;picture->units=units;
 for(int i=0;i<units;i++){picture->offsets[i]=offsets[i];picture->lengths[i]=lengths[i];}
 picture->changed=1;picture->next=0;
 return 1;
}
static int send_video(int ch,const unsigned char *data,size_t length,uint64_t stamp,uint64_t start){
 uint64_t prepare_started=now_us();
 if(selected_codec==3){size_t config_size;length=split_parameter_sets(data,length,video_message+10,LIVE_MAX,&config_size);
  if(config_size&&(config_size!=codec_config_size||memcmp(config_scratch,codec_config,config_size))){
   memcpy(codec_config,config_scratch,config_size);codec_config_size=config_size;codec_config_pending=1;
  }
  if(codec_config_pending&&codec_config_size){
   send_msg(ch,11,1,codec_config,codec_config_size);codec_config_pending=0;frames_since_config=0;
   fprintf(stderr,"AA_VIDEO_CODEC_CONFIG bytes=%zu\n",codec_config_size);
   session_trace("TX codec_config bytes=%zu poc2=%d/%d %s",codec_config_size,h264_poc2,poc_state.active,hex64(codec_config,codec_config_size));
  }
  if(!length)return 0;
  if(frames_since_config++<3){
   char types[32];size_t k=0,at=annexb_next(video_message+10,length,0);types[0]=0;
   while(at<length&&k+4<sizeof types){size_t code=video_message[10+at+2]==1?3:4;if(at+code<length)k+=(size_t)snprintf(types+k,sizeof types-k,"%u,",video_message[10+at+code]&31u);at=annexb_next(video_message+10,length,at+code);}
   session_trace("TX video after_config=%d bytes=%zu nal=%s",frames_since_config,length,types);
  }
 }else memcpy(video_message+10,data,length);
 video_message[0]=0;video_message[1]=0;
 for(int k=9;k>=2;k--){video_message[k]=(unsigned char)stamp;stamp>>=8;}
 size_t total=length+10;
 uint64_t send_started=now_us();
 uint64_t prepare_duration=send_started-prepare_started;
 if(prepare_duration>=10000)
  session_trace("VIDEO_PREPARE_SLOW bytes=%zu duration_us=%llu",total,(unsigned long long)prepare_duration);
 for(size_t offset=0;offset<total;){
  fragmented_total=(uint32_t)total;size_t chunk=total-offset;if(chunk>15000)chunk=15000;
  tx_ch=ch;tx_flags=8|(offset==0?1:0)|(offset+chunk==total?2:0);
  int r=mbedtls_ssl_write(&ssl,video_message+offset,chunk);check(r,"Video TLS write");
  if(r!=(int)chunk){fprintf(stderr,"Short video write\n");exit(1);}offset+=chunk;
  /* Complete each TLS record before letting another AA channel send. */
  if(offset<total)audio_pump(now_us(),start);
 }
 uint64_t duration=now_us()-send_started;
 if(duration>=30000)session_trace("VIDEO_SEND_SLOW bytes=%zu duration_us=%llu",total,(unsigned long long)duration);
 return 1;
}
static int send_dashboard_picture(int ch,struct dashboard_picture *picture,uint64_t start){
 int unit=picture->next;
 picture->next=(unit+1)%picture->units;
 int sent=send_video(ch,picture->video+picture->offsets[unit],picture->lengths[unit],now_us()-start,start);
 picture->last_sent=now_us();picture->changed=0;
 return sent;
}
/* CarPlay pictures decode only from an IDR on: after dropped frames the bridge waits for one and
 * asks the iPhone for it through the relay (datagram 0x10 on the touch socket). */
static int need_idr;static uint64_t keyframe_asked;
static int has_idr(const unsigned char *p,size_t n){
 size_t at=annexb_next(p,n,0);while(at<n){size_t code=p[at+2]==1?3:4;if(at+code<n&&(p[at+code]&31)==5)return 1;at=annexb_next(p,n,at+code);}return 0;
}
static void request_keyframe(void){
 if(keyframe_asked&&now_us()-keyframe_asked<1000000)return;keyframe_asked=now_us();
 const char *video_path=getenv("AA_VIDEO_SOCKET");if(!video_path)return;
 struct sockaddr_un target={0};target.sun_family=AF_UNIX;
 if(snprintf(target.sun_path,sizeof target.sun_path,"%s.touch",video_path)>=(int)sizeof target.sun_path)return;
 int ask_fd=socket(AF_UNIX,SOCK_DGRAM,0);if(ask_fd<0)return;unsigned char ask=0x10;
 if(sendto(ask_fd,&ask,1,0,(struct sockaddr*)&target,sizeof target)==1)session_trace("TX keyframe_request");
 close(ask_fd);
}
static void configure_tls(const char *certfile,const char *keyfile){
 static mbedtls_ssl_config cfg;static mbedtls_entropy_context entropy;static mbedtls_ctr_drbg_context rng;static mbedtls_x509_crt cert;static mbedtls_pk_context key;
 mbedtls_ssl_init(&ssl);mbedtls_ssl_config_init(&cfg);mbedtls_entropy_init(&entropy);mbedtls_ctr_drbg_init(&rng);mbedtls_x509_crt_init(&cert);mbedtls_pk_init(&key);
 check(mbedtls_ctr_drbg_seed(&rng,mbedtls_entropy_func,&entropy,(const unsigned char*)"aa-lab",6),"RNG");
 check(mbedtls_x509_crt_parse_file(&cert,certfile),"Certificate");check(mbedtls_pk_parse_keyfile(&key,keyfile,NULL),"Key");
 check(mbedtls_ssl_config_defaults(&cfg,MBEDTLS_SSL_IS_SERVER,MBEDTLS_SSL_TRANSPORT_STREAM,MBEDTLS_SSL_PRESET_DEFAULT),"TLS config");
 mbedtls_ssl_conf_min_version(&cfg,MBEDTLS_SSL_MAJOR_VERSION_3,MBEDTLS_SSL_MINOR_VERSION_3);
 mbedtls_ssl_conf_max_version(&cfg,MBEDTLS_SSL_MAJOR_VERSION_3,MBEDTLS_SSL_MINOR_VERSION_3);
 static const int test_ciphers[]={MBEDTLS_TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,0};
 mbedtls_ssl_conf_ciphersuites(&cfg,test_ciphers);
 mbedtls_ssl_conf_authmode(&cfg,MBEDTLS_SSL_VERIFY_NONE);mbedtls_ssl_conf_rng(&cfg,mbedtls_ctr_drbg_random,&rng);
 check(mbedtls_ssl_conf_own_cert(&cfg,&cert,&key),"Identity");check(mbedtls_ssl_setup(&ssl,&cfg),"TLS setup");mbedtls_ssl_set_bio(&ssl,NULL,tls_send,tls_recv,NULL);
}
static void *usb_events(void *unused){
 (void)unused;unsigned char events[48];
 sigset_t blocked;sigemptyset(&blocked);sigaddset(&blocked,SIGALRM);sigaddset(&blocked,SIGTERM);sigaddset(&blocked,SIGINT);pthread_sigmask(SIG_BLOCK,&blocked,NULL);
 while(!stop){ssize_t n=read(ep0,events,sizeof events);if(n<=0)break;
  for(ssize_t i=0;i+12<=n;i+=12)fprintf(stderr,"FUNCTIONFS event=%u\n",events[i+8]);
 }return NULL;
}
/* Keep a FunctionFS OUT request pending while TLS/application code runs.
 * A stream socket carries bytes in order; backpressure never discards data. */
static int reader_usb_fd, reader_queue_fd;
static void *usb_reader(void *unused){
 (void)unused;
 sigset_t blocked;sigemptyset(&blocked);sigaddset(&blocked,SIGALRM);sigaddset(&blocked,SIGTERM);sigaddset(&blocked,SIGINT);pthread_sigmask(SIG_BLOCK,&blocked,NULL);
 unsigned char buffer[16384];unsigned transfer=0;
 while(!stop){
  ssize_t n=read(reader_usb_fd,buffer,sizeof buffer);
  if(n<0&&errno==EINTR)continue;
  if(n==0){if(getenv("AA_TEST_READER"))break;continue;}
  if(n<0){fprintf(stderr,"USB reader failed errno=%d (%s)\n",errno,strerror(errno));break;}
  if(raw_capture){fwrite(buffer,1,(size_t)n,raw_capture);fflush(raw_capture);}
  TRACE("USB transfer=%u bytes=%zd\n",++transfer,n);
  size_t offset=0;
  while(offset<(size_t)n){
   ssize_t w=write(reader_queue_fd,buffer+offset,(size_t)n-offset);
   if(w<0&&errno==EINTR)continue;
   if(w<=0)goto finished;
   offset+=(size_t)w;
  }
 }
finished:
 shutdown(reader_queue_fd,SHUT_WR);return NULL;
}


#define FAST_SLOTS 32
static unsigned char fast_data[FAST_SLOTS][16384];
static size_t fast_lengths[FAST_SLOTS];
static unsigned fast_write,fast_read,fast_closed;
static pthread_mutex_t fast_mu=PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t fast_cv;
static int fast_cv_ready;
static void fast_init_sync(void){
 if(fast_cv_ready)return;
 pthread_condattr_t attr;pthread_condattr_init(&attr);
#ifdef __linux__
 pthread_condattr_setclock(&attr,CLOCK_MONOTONIC);
#endif
 pthread_cond_init(&fast_cv,&attr);pthread_condattr_destroy(&attr);
 fast_cv_ready=1;
}
static void fast_wait_50ms(void){
 struct timespec ts;
#ifdef __linux__
 clock_gettime(CLOCK_MONOTONIC,&ts);
#else
 clock_gettime(CLOCK_REALTIME,&ts);
#endif
 ts.tv_nsec+=50000000;if(ts.tv_nsec>=1000000000){ts.tv_sec++;ts.tv_nsec-=1000000000;}
 pthread_cond_timedwait(&fast_cv,&fast_mu,&ts);
}
static void *usb_fast_reader(void *unused){
 (void)unused;
 sigset_t blocked;sigemptyset(&blocked);sigaddset(&blocked,SIGALRM);sigaddset(&blocked,SIGTERM);sigaddset(&blocked,SIGINT);pthread_sigmask(SIG_BLOCK,&blocked,NULL);
 struct sched_param priority={.sched_priority=20};int rc=pthread_setschedparam(pthread_self(),SCHED_FIFO,&priority);
 fprintf(stderr,"USB_FAST_READER depth=%d realtime=%s\n",FAST_SLOTS,rc?"unavailable":"enabled");
 unsigned produced=0;
 while(!stop){
  pthread_mutex_lock(&fast_mu);
  while(!stop&&produced-fast_read==FAST_SLOTS)fast_wait_50ms();
  pthread_mutex_unlock(&fast_mu);
  if(stop)break;
  unsigned slot=produced%FAST_SLOTS;
  ssize_t n=read(reader_usb_fd,fast_data[slot],sizeof fast_data[slot]);
  if(n<0&&errno==EINTR)continue;
  if(n==0){if(getenv("AA_TEST_READER"))break;continue;}
  if(n<0){fprintf(stderr,"USB fast read errno=%d\n",errno);break;}
  fast_lengths[slot]=(size_t)n;
  pthread_mutex_lock(&fast_mu);
  fast_write=++produced;
  pthread_cond_signal(&fast_cv);
  pthread_mutex_unlock(&fast_mu);
 }
 pthread_mutex_lock(&fast_mu);fast_closed=1;pthread_cond_broadcast(&fast_cv);pthread_mutex_unlock(&fast_mu);
 return NULL;
}
static void *usb_fast_consumer(void *unused){
 (void)unused;unsigned consumed=0;
 sigset_t blocked;sigemptyset(&blocked);sigaddset(&blocked,SIGALRM);sigaddset(&blocked,SIGTERM);sigaddset(&blocked,SIGINT);pthread_sigmask(SIG_BLOCK,&blocked,NULL);
 while(!stop){
  pthread_mutex_lock(&fast_mu);
  while(!stop&&consumed==fast_write&&!fast_closed)fast_wait_50ms();
  int empty=consumed==fast_write;
  int closed=fast_closed;
  pthread_mutex_unlock(&fast_mu);
  if(empty){if(closed||stop)break;continue;}
  unsigned slot=consumed%FAST_SLOTS;size_t n=fast_lengths[slot];
  if(raw_capture){fwrite(fast_data[slot],1,n,raw_capture);fflush(raw_capture);}
  size_t offset=0;
  while(offset<n){ssize_t w=write(reader_queue_fd,fast_data[slot]+offset,n-offset);if(w<0&&errno==EINTR)continue;if(w<=0)goto done;offset+=(size_t)w;}
  pthread_mutex_lock(&fast_mu);
  fast_read=++consumed;
  pthread_cond_signal(&fast_cv);
  pthread_mutex_unlock(&fast_mu);
 }
done:shutdown(reader_queue_fd,SHUT_WR);return NULL;
}
#define AIO_DEPTH 8
static aio_context_t aio_ctx;
static struct iocb aio_requests[AIO_DEPTH];
static unsigned char aio_buffers[AIO_DEPTH][16384];
static void aio_submit_slot(unsigned slot){
 struct iocb *cb=&aio_requests[slot];memset(cb,0,sizeof *cb);
 cb->aio_data=slot;cb->aio_lio_opcode=IOCB_CMD_PREAD;cb->aio_fildes=reader_usb_fd;
 cb->aio_buf=(uintptr_t)aio_buffers[slot];cb->aio_nbytes=sizeof aio_buffers[slot];
 struct iocb *list[]={cb};
 long n;do{n=syscall(SYS_io_submit,aio_ctx,1,list);}while(n<0&&errno==EINTR);
 if(n!=1)die("FunctionFS AIO submit");
}
static void *usb_aio_reader(void *unused){
 (void)unused;unsigned next=0;int ready[AIO_DEPTH]={0};long sizes[AIO_DEPTH]={0};
 sigset_t blocked;sigemptyset(&blocked);sigaddset(&blocked,SIGALRM);sigaddset(&blocked,SIGTERM);sigaddset(&blocked,SIGINT);pthread_sigmask(SIG_BLOCK,&blocked,NULL);
 for(unsigned i=0;i<AIO_DEPTH;i++)aio_submit_slot(i);
 while(!stop){
  struct io_event events[AIO_DEPTH];long n=syscall(SYS_io_pgetevents_time64,aio_ctx,1,AIO_DEPTH,events,NULL,NULL);
  if(n<0&&errno==EINTR)continue;if(n<0){perror("FunctionFS AIO completion");break;}
  for(long i=0;i<n;i++){
   unsigned slot=(unsigned)events[i].data;
   if(slot>=AIO_DEPTH||ready[slot]){fprintf(stderr,"AIO invalid completion slot\n");goto done;}
   ready[slot]=1;sizes[slot]=(long)events[i].res;
  }
  while(ready[next]){
   long size=sizes[next];if(size<0){fprintf(stderr,"AIO USB completion error=%ld\n",size);goto done;}
   unsigned char copy[16384];if(size>(long)sizeof copy)goto done;
   memcpy(copy,aio_buffers[next],(size_t)size);ready[next]=0;
   aio_submit_slot(next);next=(next+1)%AIO_DEPTH;
   if(raw_capture){fwrite(copy,1,(size_t)size,raw_capture);fflush(raw_capture);}
   fprintf(stderr,"USB AIO bytes=%ld queued=%d\n",size,AIO_DEPTH);
   size_t offset=0;
   while(offset<(size_t)size){ssize_t w=write(reader_queue_fd,copy+offset,(size_t)size-offset);if(w<0&&errno==EINTR)continue;if(w<=0)goto done;offset+=(size_t)w;}
  }
 }
done:
 shutdown(reader_queue_fd,SHUT_WR);return NULL;
}
static void start_usb_reader(void){
 int use_aio=getenv("AA_USB_AIO")!=NULL;
 int queue[2];if(socketpair(AF_UNIX,SOCK_STREAM,0,queue))die("USB receive queue");
 reader_usb_fd=fd;reader_queue_fd=queue[1];fd=queue[0];
 /* The queue is a stream: a zero-byte read means EOF, not a USB ZLP. */
 buffered_test=1;
 if(use_aio){
  if(syscall(SYS_io_setup,AIO_DEPTH,&aio_ctx)<0)die("FunctionFS AIO setup");
  fprintf(stderr,"USB_AIO_READER depth=%d enabled\n",AIO_DEPTH);
 }
 if(getenv("AA_USB_FAST_READ")){
  fast_init_sync();
  memset(fast_data,0,sizeof fast_data);
  pthread_t consumer;int err=pthread_create(&consumer,NULL,usb_fast_consumer,NULL);if(err){errno=err;die("USB ring consumer");}pthread_detach(consumer);
 }
 pthread_t thread;int rc=pthread_create(&thread,NULL,getenv("AA_USB_FAST_READ")?usb_fast_reader:use_aio?usb_aio_reader:usb_reader,NULL);if(rc){errno=rc;die("USB reader thread");}pthread_detach(thread);
 fprintf(stderr,"USB_CONTINUOUS_READER enabled\n");
}
static void open_ffs(const char *mountpoint){
 char path[256];if(strlen(mountpoint)>220){fprintf(stderr,"FFS path too long\n");exit(2);}
 snprintf(path,sizeof path,"%s/ep0",mountpoint);ep0=open(path,O_RDWR);if(ep0<0)die("FunctionFS ep0");
 /* v2 header, FS+HS descriptor counts, interface and two bulk endpoints. */
 struct __attribute__((packed)) {
  uint32_t magic,length,flags,fs_count,hs_count;
  unsigned char fs[23],hs[23];
 } desc={3,sizeof desc,3,3,3,
  {9,4,0,0,2,255,255,0,1, 7,5,0x81,2,64,0,0, 7,5,2,2,64,0,0},
  {9,4,0,0,2,255,255,0,1, 7,5,0x81,2,0,2,0, 7,5,2,2,0,2,0}};
 if(write(ep0,&desc,sizeof desc)!=(ssize_t)sizeof desc)die("FunctionFS descriptors");
 struct __attribute__((packed)){uint32_t magic,length,count,langs;uint16_t lang;char name[15];}
 strings={2,sizeof strings,1,1,0x409,"CatPlay AA Lab"};
 if(write(ep0,&strings,sizeof strings)!=(ssize_t)sizeof strings)die("FunctionFS strings");
 snprintf(path,sizeof path,"%s/ep1",mountpoint);out_fd=open(path,O_WRONLY);if(out_fd<0)die("FunctionFS IN");
 snprintf(path,sizeof path,"%s/ep2",mountpoint);fd=open(path,O_RDONLY);if(fd<0)die("FunctionFS OUT");
 pthread_t thread;int rc=pthread_create(&thread,NULL,usb_events,NULL);if(rc){errno=rc;die("USB event thread");}pthread_detach(thread);
 start_usb_reader();
 fprintf(stderr,"FUNCTIONFS_READY\n");
}
static int run_mtp(const char *device){
 fd=open(device,O_RDWR);if(fd<0)die("MTP device");out_fd=fd;usb_mode=1;
 signal(SIGPIPE,SIG_IGN);
 struct sigaction action;memset(&action,0,sizeof action);action.sa_handler=interrupted;sigemptyset(&action.sa_mask);
 sigaction(SIGTERM,&action,NULL);sigaction(SIGINT,&action,NULL);
 uint32_t session=0;fprintf(stderr,"MTP_READY: discovery only, no storage exposed\n");
 while(!stop){
  unsigned char command[32];struct mtp_reply reply;
  if(all(0,command,12))break;
  size_t count=mtp_u32(command);
  if(count<12 || count>sizeof command || count%4){fprintf(stderr,"MTP_INVALID_CONTAINER length=%zu\n",count);break;}
  if(count>12 && all(0,command+12,count-12))break;
  if(mtp_command(command,count,&session,&reply)){fprintf(stderr,"MTP_INVALID_COMMAND\n");break;}
  fprintf(stderr,"MTP_RX op=%04x transaction=%u response=%04x data=%zu\n",mtp_u16(command+6),mtp_u32(command+8),mtp_u16(reply.response+6),reply.data_size);
  if(reply.data_size && all(1,reply.data,reply.data_size))break;
  if(all(1,reply.response,reply.response_size))break;
 }
 close(fd);fprintf(stderr,"MTP_CLOSED: disconnect or switch to accessory\n");return 0;
}
static int gadget_flag(const char *name){
 char path[512],value=0;const char *dir=getenv("AA_TEST_USB_STATE_DIR");
 if(!dir)dir="/sys/module/aa_bulk/parameters";
 if(snprintf(path,sizeof path,"%s/%s",dir,name)>=(int)sizeof path)return -1;
 int f=open(path,O_RDONLY);if(f<0)return -1;ssize_t n=read(f,&value,1);close(f);
 return n==1?(value=='Y'||value=='1'):-1;
}
static int wait_mtp_accessory(const char *device){
 fprintf(stderr,"MTP_DISCOVERY: waiting for head-unit ACCESSORY_START\n");
 while(!stop){
  int accessory=gadget_flag("accessory_active"),online=gadget_flag("usb_online");
  if(accessory<0 || online<0){fprintf(stderr,"MTP_STATE_UNAVAILABLE\n");return -1;}
  if(accessory && online){usb_pos=usb_len=0;fprintf(stderr,"MTP_SWITCHED_TO_ACCESSORY: USB endpoints ready\n");return 0;}
  if(!accessory && online){run_mtp(device);usb_pos=usb_len=0;}
  else usleep(100000);
 }
 return -1;
}
static int wait_usb_online(void){
 int online=gadget_flag("usb_online");
 if(online<0||online)return 0;
 fprintf(stderr,"USB_WAIT_ONLINE: waiting for head-unit USB enumeration\n");
 while(!stop){
  online=gadget_flag("usb_online");
  if(online<0||online){usb_pos=usb_len=0;fprintf(stderr,"USB_ONLINE: opening bulk endpoints\n");return 0;}
  usleep(25000);
 }
 return -1;
}
int main(int argc,char **argv){
 if(argc==3 && !strcmp(argv[1],"--mtp"))return run_mtp(argv[2]);
 if(argc==2&&!strcmp(argv[1],"--probe-aio")){
  aio_context_t ctx=0;
  if(syscall(SYS_io_setup,8,&ctx)<0){fprintf(stderr,"AIO_UNAVAILABLE errno=%d (%s): kernel support required\n",errno,strerror(errno));return 3;}
  syscall(SYS_io_destroy,ctx);fprintf(stderr,"AIO_AVAILABLE\n");return 0;
 }
 if(argc!=6){fprintf(stderr,"Usage: %s HOST PORT CERT KEY MIRE.h264\n       %s --ffs MOUNT CERT KEY MIRE.h264\n",argv[0],argv[0]);return 2;}
 audio_preroll_us=parse_audio_preroll(getenv("AA_AUDIO_PREROLL"));
 diagnostic=getenv("AA_DIAGNOSTIC")&&!strcmp(getenv("AA_DIAGNOSTIC"),"1");h264_poc2=h264_poc2_enabled();
 if(diagnostic){
  raw_capture=fopen("usb-rx.bin","wb");frame_capture=fopen("aa-rx.bin","wb");
  if(!raw_capture||!frame_capture)die("Diagnostic capture");
 }
 fprintf(stderr,"AA_MODE=%s\n",diagnostic?"diagnostic":"production");
 signal(SIGPIPE,SIG_IGN);
 if(getenv("AA_VIDEO_SOCKET"))open_live_input(getenv("AA_VIDEO_SOCKET"));
 struct sigaction action;memset(&action,0,sizeof action);action.sa_handler=interrupted;sigemptyset(&action.sa_mask);
 sigaction(SIGINT,&action,NULL);sigaction(SIGTERM,&action,NULL);sigaction(SIGALRM,&action,NULL);
 if(!getenv("AA_AUTONOMOUS"))alarm(live_listener>=0?630:150);setvbuf(stderr,NULL,_IONBF,0);
 unsigned char video[15000];FILE *f=fopen(argv[5],"rb");if(!f)die("Video file");size_t vn=fread(video,1,sizeof video,f);if(!vn||!feof(f)){fprintf(stderr,"Test frame must be 1..14999 bytes\n");return 2;}fclose(f);
 int raw_mode=!strcmp(argv[1],"--raw");
 usb_mode=!strcmp(argv[1],"--ffs")||raw_mode;
 if(raw_mode && getenv("AA_MTP_DISCOVERY") && wait_mtp_accessory(argv[2]))return 1;
 if(raw_mode && !getenv("AA_MTP_DISCOVERY") && wait_usb_online())return 1;
 if(raw_mode){fd=open(argv[2],O_RDWR);if(fd<0)die("Kernel queue device");out_fd=fd;start_usb_reader();}
 else if(usb_mode)open_ffs(argv[2]);
 else {
 struct addrinfo hints={0},*ai;hints.ai_socktype=SOCK_STREAM;hints.ai_family=AF_INET;
 int err=getaddrinfo(argv[1],argv[2],&hints,&ai);if(err){fprintf(stderr,"getaddrinfo: %s\n",gai_strerror(err));return 1;}
 fd=socket(ai->ai_family,ai->ai_socktype,0);if(fd<0)die("socket");
 if(getenv("AA_TCP_LISTEN")){
  int yes=1;setsockopt(fd,SOL_SOCKET,SO_REUSEADDR,&yes,sizeof yes);
  if(bind(fd,ai->ai_addr,ai->ai_addrlen)||listen(fd,1))die("TCP listen");
  fprintf(stderr,"AA_TCP_READY\n");int client=accept(fd,NULL,NULL);if(client<0)die("TCP accept");close(fd);fd=client;
 }else if(connect(fd,ai->ai_addr,ai->ai_addrlen))die("connect");
 freeaddrinfo(ai);out_fd=fd;if(getenv("AA_TEST_BUFFERED")){usb_mode=1;buffered_test=1;}
 struct timeval timeout={10,0};setsockopt(fd,SOL_SOCKET,SO_RCVTIMEO,&timeout,sizeof timeout);setsockopt(fd,SOL_SOCKET,SO_SNDTIMEO,&timeout,sizeof timeout);
 if(getenv("AA_TEST_READER")){usb_mode=1;start_usb_reader();}
 }
 open_session_trace();session_trace("SESSION_START pid=%d transport=%s",(int)getpid(),argv[1]);
 int ch,flags;size_t n;if(receive(&ch,&flags,wire,&n)||ch||n<6||wire[0]||wire[1]!=1){session_trace("NO_VERSION_REQUEST");fprintf(stderr,"Expected AA VERSION_REQUEST\n");return 1;}
 session_trace("RX version %s",hex16(wire,n));
 unsigned char version[]={0,2,0,1,0,7,0,0};if(frame(0,3,version,sizeof version))die("VERSION_RESPONSE");fprintf(stderr,"AA VERSION 1.7 negotiated\n");
 configure_tls(argv[3],argv[4]);check(mbedtls_ssl_handshake(&ssl),"TLS handshake");if(flush_handshake())die("TLS flight write");handshake=0;fprintf(stderr,"AA TLS 1.2 established cipher=%s\n",mbedtls_ssl_get_ciphersuite(&ssl));
 session_trace("TLS_OK");
 int video_ch=-1,running=0,focused=0,unacked=0,max_unacked=1,frames=0,lost_focus=0;
 unsigned video_acks=0;
 uint64_t input_offer=0;
 uint64_t start=now_us(),next_test=0;
 int dashboard_enabled=live_listener>=0&&getenv("AA_DASHBOARD_DIR")!=NULL;
 enum projection_screen screen=initial_projection_screen(dashboard_enabled);
 /* The Uconnect returns to its native screen when no picture arrives ~5 s after granting focus, and
  * keeps the first codec configuration of a session: the waiting screen is encoded under the
  * iPhone's own SPS/PPS (tooling/waiting_iphone_config.py) and starts at once. */
 uint64_t waiting_delay=getenv("AA_WAITING_DELAY_MS")?strtoull(getenv("AA_WAITING_DELAY_MS"),NULL,10)*1000:0;
 uint64_t next_waiting=start+waiting_delay;
 while(!stop&&(getenv("AA_AUTONOMOUS")||now_us()-start<(live_listener>=0?600000000ULL:120000000ULL))){
  poll_live_disconnect();
   if(live_disconnected){
    live_disconnected=0;unacked=0;need_idr=1;codec_config_pending=1;
    screen=projection_after_disconnect(screen,dashboard_enabled);
    next_waiting=0;menu_picture.changed=1;
    unsigned char stale[8192];while(audio_fd>=0&&recv(audio_fd,stale,sizeof stale,MSG_DONTWAIT)>0){}
    while(guidance_fd>=0&&recv(guidance_fd,stale,sizeof stale,MSG_DONTWAIT)>0){}
    audio_stop("carplay_disconnected");
    guidance_stop("carplay_disconnected");
    mic_wanted=0;microphone_request();
    const char *destination=screen==PROJECTION_MENU?"menu":"waiting";
    fprintf(stderr,"CARPLAY_DISCONNECTED: returning to %s\n",destination);
    session_trace("CARPLAY_DISCONNECTED screen=%s",destination);
   }
  if(control_fd>=0&&video_ch>0){unsigned char ctrl[512];
   ssize_t cn=recv(control_fd,ctrl,sizeof ctrl,MSG_DONTWAIT);
   if(cn>=8&&ctrl[0]==0x30){
    media_metadata_on_control(ctrl,(size_t)cn);
   }else if(cn==1){
    unsigned char event=ctrl[0];
    if(event==2||event==3){mic_wanted=event==2;session_trace("AA_MIC_CTRL wanted=%d ready=%d ch=%d open=%d",mic_wanted,mic_ready,mic_channel,mic_open);microphone_request();}
    else if(event==4){if(live_peer>=0)close(live_peer);live_peer=-1;live_used=0;live_disconnected=1;}
    else if(dashboard_enabled&&(event==1||(event==8&&screen==PROJECTION_WAITING))){
     screen=PROJECTION_MENU;dashboard_visible=DASHBOARD_NONE;menu_picture.changed=1;
     codec_config_pending=1;
     session_trace(event==8?"CARPLAY_WAIT_CANCELLED":"DASHBOARD_OPEN");
    }else if((event==5||event==7)&&screen==PROJECTION_MENU){
     screen=PROJECTION_WAITING;dashboard_visible=0;next_waiting=0;
     need_idr=1;codec_config_pending=1;
     if(event==7&&live_peer>=0){close(live_peer);live_peer=-1;live_used=0;live_disconnected=1;}
     if(live_peer>=0)request_keyframe();
     session_trace("CARPLAY_WAITING restart=%d",event==7);
    }else if(event==1||(event==6&&dashboard_visible==DASHBOARD_MENU)){
     unsigned char native[]={0x10,2,0x18,4};
     send_msg(video_ch,11,0x8007,native,sizeof native);
     focused=0;dashboard_visible=0;
     session_trace("CAR_RETURN native focus requested");
    }
   }
  }
  {uint64_t pump_now=now_us();audio_pump(pump_now,start);guidance_pump(pump_now,start);}
  /* Without video focus the relay must still be drained, or its 2 s write timeout ends CarPlay video. */
  if(live_listener>=0&&(!(running&&focused)||screen==PROJECTION_MENU)){size_t dl;uint64_t ds;unsigned dropped=0;
   while(dropped<8&&next_live_frame(&dl,&ds)){live_used=0;dropped++;}
   if(dropped){if(!need_idr)session_trace("VIDEO_DROP hidden frames=%u",dropped);need_idr=1;}
  }
  if(running&&focused&&unacked<max_unacked&&(live_listener>=0||now_us()>=next_test)){
   size_t length=vn;uint64_t stamp=now_us()-start;
   int live_ready=live_listener>=0&&screen!=PROJECTION_MENU?next_live_frame(&length,&stamp):0;
   if(live_ready&&screen==PROJECTION_WAITING)need_idr=1;
   if(live_ready&&need_idr){
    if(has_idr(live_buf+12,length)){need_idr=0;session_trace("VIDEO_RESUME_IDR bytes=%zu",length);}
    else{live_used=0;live_ready=0;request_keyframe();}
   }
   if(live_listener<0||live_ready){
    /* One monotonic session clock for every source: CarPlay PTS restart near zero, which the
     * Uconnect renderer treats as late samples after the waiting screen and never displays. */
    if(live_ready&&screen==PROJECTION_WAITING)codec_config_pending=1;
    int sent=send_video(video_ch,live_listener<0?video:live_buf+12,length,now_us()-start,start);
    if(live_ready){
     live_used=0;
     if(sent){
      if(screen==PROJECTION_WAITING)session_trace("CARPLAY_FIRST_FRAME after_waiting=%d",frames>0);
      screen=PROJECTION_CARPLAY;dashboard_visible=0;
     }
    }else next_test=now_us()+500000;
    unacked+=sent;frames+=sent;
    TRACE("VIDEO frame=%d bytes=%zu unacked=%d source=%s\n",frames,length,unacked,live_listener<0?"test":"CarPlay");
    if(sent&&(frames<=5||!(frames%50)))session_trace("TX video n=%d bytes=%zu unacked=%d src=%s",frames,length,unacked,live_listener<0?"test":"CarPlay");
   }else if(screen==PROJECTION_MENU&&dashboard_refresh(&menu_picture,"dashboard")&&
            (menu_picture.changed||now_us()-menu_picture.last_sent>=1000000)){
    int sent=send_dashboard_picture(video_ch,&menu_picture,start);
    if(sent&&dashboard_visible!=DASHBOARD_MENU)session_trace("DASHBOARD_VISIBLE");
    if(sent)dashboard_visible=DASHBOARD_MENU;
    unacked+=sent;frames+=sent;
   }else if(screen==PROJECTION_WAITING&&now_us()>=next_waiting&&dashboard_enabled&&
            dashboard_refresh(&wait_picture,"dashboard-wait")){
    int sent=send_dashboard_picture(video_ch,&wait_picture,start);
    if(sent&&dashboard_visible!=DASHBOARD_WAITING)session_trace("DASHBOARD_WAIT_VISIBLE");
    if(sent)dashboard_visible=DASHBOARD_WAITING;
    unacked+=sent;frames+=sent;next_waiting=now_us()+1000000;
   }else if(!dashboard_enabled&&waiting_picture_due(screen,now_us(),next_waiting)){
    /* A single coded picture is resent only with a new codec configuration. */
    int unit=waiting_next;waiting_next=(waiting_next+1)%waiting_units;
    int sent=send_video(video_ch,waiting_video+waiting_unit_off[unit],waiting_unit_len[unit],now_us()-start,start);
    unacked+=sent;frames+=sent;next_waiting=now_us()+1000000;
    TRACE("VIDEO frame=%d bytes=%zu source=Bluetooth-waiting-screen\n",frames,waiting_unit_len[unit]);
    if(sent&&(frames<=5||!(frames%50)))session_trace("TX video n=%d bytes=%zu unacked=%d src=waiting unit=%d",frames,waiting_unit_len[unit],unacked,unit);
   }
  }
  fd_set set;FD_ZERO(&set);FD_SET(fd,&set);struct timeval wait={0,100000};
  if(!mbedtls_ssl_get_bytes_avail(&ssl)&&!left){
   if(!(usb_mode&&usb_pos<usb_len)){
    int max_fd=fd;if(control_fd>=0){FD_SET(control_fd,&set);if(control_fd>max_fd)max_fd=control_fd;}int video_fd=live_peer>=0?live_peer:live_listener;
    uint64_t select_now=now_us();
    if(audio_prime_until>select_now)wait.tv_usec=(long)(audio_prime_until-select_now);
    if(audio_prime_until<=select_now&&audio_ready&&audio_unacked<audio_max_unacked&&audio_fd>=0){FD_SET(audio_fd,&set);if(audio_fd>max_fd)max_fd=audio_fd;}
    if(guidance_ready&&guidance_unacked<guidance_max_unacked&&guidance_fd>=0){FD_SET(guidance_fd,&set);if(guidance_fd>max_fd)max_fd=guidance_fd;}
    /* A readable video socket cannot make progress while the AA window is full.
     * Stop waking on it until an ACK arrives on the USB input. */
    if(video_fd>=0&&(screen==PROJECTION_MENU||!(running&&focused)||unacked<max_unacked)){
     FD_SET(video_fd,&set);
     if(video_fd>max_fd)max_fd=video_fd;
    }
    if(select(max_fd+1,&set,NULL,NULL,&wait)<=0)continue;
    if(!FD_ISSET(fd,&set))continue;
   }
   int incoming_flags;size_t incoming_len;
   if(receive(&rx_ch,&incoming_flags,incoming,&incoming_len)){session_trace("TRANSPORT_CLOSED");fprintf(stderr,"AA_TRANSPORT_CLOSED frames=%d\n",frames);break;}
   if(!(incoming_flags&8)){
    session_trace("RX plain ch=%d flags=%02x len=%zu %s",rx_ch,incoming_flags,incoming_len,hex16(incoming,incoming_len));
    if(rx_ch==0&&incoming_len>=2&&incoming[0]==0&&incoming[1]==4){
     if(value(incoming+2,incoming_len-2,1,999)!=0){fprintf(stderr,"AA authentication rejected\n");return 1;}
     unsigned char name[]={0x22,10,'C','a','r','F','l','u','x','o','i','d'};
     send_msg(0,11,5,name,sizeof name);
    }
    continue;
   }
   at=0;left=incoming_len;
  }
  if(diagnostic){fprintf(stderr,"TLS_EXPECTED_RX_COUNTER=");for(int k=0;k<8;k++)fprintf(stderr,"%02x",ssl.in_ctr[k]);fprintf(stderr,"\n");}
  int r=mbedtls_ssl_read(&ssl,plain,sizeof plain);if(r==0)break;if(r==MBEDTLS_ERR_SSL_WANT_READ||r==MBEDTLS_ERR_SSL_WANT_WRITE)continue;check(r,"TLS read");if(r<2)continue;
  int id=(plain[0]<<8)|plain[1];const unsigned char *body=plain+2;size_t len=(size_t)r-2;TRACE("RX ch=%d msg=%04x bytes=%zu\n",rx_ch,id,len);
  if(rx_ch==video_ch&&id==0x8004){if(++video_acks<=5||!(video_acks%50))session_trace("RX video ack n=%u",video_acks);}
  else if(!(rx_ch==mic_channel&&id<2)&&!(rx_ch==audio_channel&&(id==0x8004||id==0x800b))&&!(rx_ch==guidance_channel&&(id==0x8004||id==0x800b))&&!(rx_ch==input_channel&&id==0x8001)&&!(rx_ch==sensor_channel&&id==0x8003)&&!(rx_ch==0&&id==0x0b&&ping_seen++%10))session_trace("RX ch=%d id=%04x len=%zu %s",rx_ch,id,len,hex16(body,len));
  if(rx_ch==0&&id==4){unsigned char name[]={0x22,10,'C','a','r','F','l','u','x','o','i','d'};send_msg(0,11,5,name,sizeof name);}
  else if(rx_ch==0&&id==6){char bt_mac[18]={0};input_channel=find_input(body,len,&input_offer);video_ch=find_video(body,len);audio_channel=live_listener>=0?find_media_audio(body,len):-1;guidance_channel=live_listener>=0?find_guidance_audio(body,len):-1;mic_channel=live_listener>=0?find_microphone(body,len):-1;media_status_channel=live_listener>=0?find_media_status(body,len):-1;sensor_channel=live_listener>=0?find_sensor(body,len,&sensor_mask):-1;int has_bt=live_listener>=0&&find_bluetooth_mac(body,len,bt_mac);if(live_listener>=0&&selected_codec!=3){fprintf(stderr,"Live CarPlay requires an H264 receiver\n");return 1;}if(video_ch<0){fprintf(stderr,"No H264 video service\n");return 1;}fprintf(stderr,"Selected video service=%d codec=%d AA_DISPLAY=%ux%u\n",video_ch,selected_codec,display_width,display_height);
 load_waiting_video();index_waiting_units();session_trace("SERVICES video=%d input=%d audio=%d guidance=%d mic=%d media_st=%d sensor=%d(0x%x) bt=%s waiting_units=%d margins=%ux%u",video_ch,input_channel,audio_channel,guidance_channel,mic_channel,media_status_channel,sensor_channel,sensor_mask,has_bt?bt_mac:"none",waiting_units,video_margin_w,video_margin_h);
 for(size_t o=0;o<len;o+=48)session_trace("SDR %zu %s",o,hex64(body+o,len-o<48?len-o:48));
 if(getenv("AA_VIDEO_SOCKET")){char path[4096];snprintf(path,sizeof path,"%s.display",getenv("AA_VIDEO_SOCKET"));FILE *display=fopen(path,"w");if(!display)die("Display negotiation file");fprintf(display,"%u %u %u %u\n",display_width,display_height,video_margin_w,video_margin_h);fclose(display);
  if(has_bt){snprintf(path,sizeof path,"%s.btmac",getenv("AA_VIDEO_SOCKET"));FILE *bf=fopen(path,"w");if(bf){fprintf(bf,"%s\n",bt_mac);fclose(bf);}fprintf(stderr,"AA_BT_MAC mac=%s\n",bt_mac);}
 }if(selected_codec==7){char path[4096];snprintf(path,sizeof path,"%s.hevc",argv[5]);FILE *hf=fopen(path,"rb");if(!hf)die("HEVC test frame");vn=fread(video,1,sizeof video,hf);if(!vn||!feof(hf)){fclose(hf);return 1;}fclose(hf);}open_video(video_ch);if(input_channel>0)open_video(input_channel);if(audio_channel>0)open_video(audio_channel);if(guidance_channel>0)open_video(guidance_channel);if(mic_channel>0)open_video(mic_channel);if(media_status_channel>0)open_video(media_status_channel);if(sensor_channel>0)open_video(sensor_channel);}
  else if(rx_ch==0&&id==11){if(ping_seen==1)fprintf(stderr,"AA_PING_RX active\n");send_msg(0,11,12,body,len);}
  else if(rx_ch==0&&id==0x13)audio_on_focus_at((unsigned)value(body,len,1,0),now_us());
  else if(rx_ch==0&&id==15){fprintf(stderr,"AA_BYEBYE_RX reason=%llu\n",(unsigned long long)value(body,len,1,0));send_msg(0,11,16,"",0);break;}
  else if(rx_ch==mic_channel&&id==8){
   /* Uconnect VP2 rejects 0x8000 SETUP on MediaSourceService with 0x00ff; channel is ready directly on OPEN_RESPONSE. */
   if(value(body,len,1,999)==0){
    mic_ready=1;
    fprintf(stderr,"AA_MIC_CHANNEL_READY\n");
    session_trace("AA_MIC_CHANNEL_READY ch=%d",mic_channel);
    microphone_request();
   }else{
    fprintf(stderr,"AA_MIC channel rejected\n");
    session_trace("AA_MIC_CHANNEL_REJECTED %s",hex16(body,len));
   }
  }
  else if(rx_ch==mic_channel&&id==0x8003){
   if(value(body,len,1,999)==2){
    mic_ready=1;
    fprintf(stderr,"AA_MIC_READY\n");
    session_trace("AA_MIC_READY %s",hex16(body,len));
    microphone_request();
   }else{
    fprintf(stderr,"AA_MIC setup rejected\n");
    session_trace("AA_MIC_SETUP_REJECTED %s",hex16(body,len));
   }
  }
  else if(rx_ch==mic_channel&&id==0x8006){microphone_on_open_response(body,len);}
  else if(rx_ch==mic_channel&&id==0x8001){mic_session=(int)value(body,len,1,1);fprintf(stderr,"AA_MIC_STARTED session=%d\n",mic_session);session_trace("AA_MIC_STARTED session=%d",mic_session);}
  else if(rx_ch==mic_channel&&(id==0||id==1)){microphone_pcm(body,len,id==0);unsigned char ack[8]={8};size_t an=1;unsigned session=(unsigned)mic_session;do{ack[an++]=(session&127)|(session>=128?128:0);session>>=7;}while(session);ack[an++]=16;ack[an++]=1;send_msg(mic_channel,11,0x8004,ack,an);}
  else if(rx_ch==mic_channel&&id==0x8002){mic_open=0;fprintf(stderr,"AA_MIC_STOPPED\n");session_trace("AA_MIC_STOPPED");}
  else if(rx_ch==media_status_channel&&id==8){
   if(value(body,len,1,999)==0){
    media_status_ready=1;
    fprintf(stderr,"AA_MEDIA_STATUS_READY ch=%d\n",media_status_channel);
    session_trace("AA_MEDIA_STATUS_READY ch=%d",media_status_channel);
    if(media_meta_len)send_msg(media_status_channel,11,0x8003,media_meta_buf,media_meta_len);
    if(audio_streaming)media_status_update(2);
   }
  }
  else if(rx_ch==media_status_channel&&id==0x8002){
   unsigned input_ev=(unsigned)value(body,len,1,0);
   unsigned char btn=input_ev==1?1:input_ev==2?2:input_ev==3?5:input_ev==4?4:0;
   int fwd=0;
   if(btn){unsigned char rep[]={0x20,btn};fwd=forward_input_report(rep,sizeof rep);}
   fprintf(stderr,"AA_MEDIA_INPUT ev=%u btn=%u fwd=%d hex=%s\n",input_ev,btn,fwd,hex16(body,len));
   session_trace("AA_MEDIA_INPUT ev=%u btn=%u fwd=%d hex=%s",input_ev,btn,fwd,hex16(body,len));
  }
  else if(rx_ch==guidance_channel&&id==8){unsigned char setup[]={8,1};send_msg(guidance_channel,11,0x8000,setup,sizeof setup);}
  else if(rx_ch==guidance_channel&&id==0x8003)guidance_on_config(body,len);
  else if(rx_ch==guidance_channel&&id==0x8004)guidance_on_ack(body,len);
  else if(rx_ch==guidance_channel&&id==0x800b){if(guidance_underflows++%25==0)session_trace("RX guidance underflow n=%u sent=%u acks=%u",guidance_underflows,guidance_sent,guidance_acks);}
  else if(rx_ch==sensor_channel&&id==8){
   if(value(body,len,1,999)==0)sensor_subscribe();
   else session_trace("AA_SENSOR_REJECTED %s",hex16(body,len));
  }
  else if(rx_ch==sensor_channel&&id==0x8002){
   int32_t status=(int32_t)(uint32_t)value(body,len,1,0);
   session_trace("AA_SENSOR_RSP status=%d",status);
   if(status)fprintf(stderr,"AA_SENSOR_RSP status=%d\n",status);
  }
  else if(rx_ch==sensor_channel&&id==0x8003){
   parse_sensor_batch(body,len);
  }
  else if(rx_ch==audio_channel&&id==8){unsigned char setup[]={8,1};send_msg(audio_channel,11,0x8000,setup,sizeof setup);}
  else if(rx_ch==audio_channel&&id==0x8003)audio_on_config(body,len);
  else if(rx_ch==audio_channel&&id==0x8004)audio_on_ack(body,len);
  else if(rx_ch==audio_channel&&id==0x800b){if(audio_underflows++%25==0)session_trace("RX audio underflow n=%u sent=%u acks=%u",audio_underflows,audio_sent,audio_acks);}
  else if(rx_ch==input_channel&&id==8){
   if(value(body,len,1,999)!=0){fprintf(stderr,"Input channel rejected\n");return 1;}
   unsigned char bindings[4*INPUT_BINDING_COUNT+8];
   size_t size=encode_input_bindings(bindings,input_offer);
   session_trace("AA_INPUT_BIND_TX ch=%d bytes=%zu mask=0x%llx",input_channel,size,(unsigned long long)input_offer);
   send_msg(input_channel,11,0x8002,bindings,size);
  }
  else if(rx_ch==input_channel&&id==0x8003){
   int32_t status=(int32_t)(uint32_t)value(body,len,1,0);
   session_trace("AA_INPUT_BINDING_STATUS status=%d",status);
   if(status)fprintf(stderr,"AA_INPUT_BINDING_REJECTED status=%d\n",status);
  }
  else if(rx_ch==input_channel&&id==0x8001){forward_car_controls(body,len);forward_touch(body,len);}
  else if(rx_ch==video_ch&&id==8){if(value(body,len,1,999)!=0){fprintf(stderr,"Channel open rejected\n");return 1;}unsigned char setup[]={8,(unsigned char)selected_codec};send_msg(video_ch,11,0x8000,setup,sizeof setup);}
  else if(rx_ch==video_ch&&id==0x8003){if(value(body,len,1,0)!=2){fprintf(stderr,"Video not ready\n");return 1;}uint64_t announced=value(body,len,2,1);max_unacked=video_window(announced);session_trace("VIDEO_READY window=%d announced=%llu",max_unacked,(unsigned long long)announced);unsigned char begin[]={8,1,16,(unsigned char)selected_video_config},focus[]={16,1};send_msg(video_ch,11,0x8001,begin,sizeof begin);send_msg(video_ch,11,0x8007,focus,sizeof focus);running=1;}
  else if(rx_ch==video_ch&&id==0x8008){focused=value(body,len,1,0)==1;
   if(!focused){codec_config_pending=1;lost_focus=frames>0;}
   else if(lost_focus){
    /* Back from the native screen the Uconnect waits for a new video SETUP (its focus manager enters
     * WAITING_AA_GAL_VIDEO_SETUP); CONFIG then restarts the stream from a fresh IDR. */
    lost_focus=0;running=0;unacked=0;codec_config_pending=1;need_idr=live_listener>=0;
    unsigned char setup[]={8,(unsigned char)selected_codec};send_msg(video_ch,11,0x8000,setup,sizeof setup);session_trace("VIDEO_RESETUP");
    if(need_idr)request_keyframe();
   }
   fprintf(stderr,"AA_VIDEO_FOCUS %s\n",focused?"projected":"native");
   session_trace("VIDEO_FOCUS %s",focused?"projected":"native");
  }
  else if(rx_ch==video_ch&&id==0x8004){int ack=(int)value(body,len,2,1);unacked=ack>unacked?0:unacked-ack;}
 }
 mic_wanted=0;microphone_request();
 session_trace("SESSION_END frames=%d stop=%d",frames,(int)stop);
 fprintf(stderr,"AA test finished: %d frames emitted\n",frames);close(fd);return frames>0?0:1;
}
