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
 if(trace_fd<0||trace_lines>=800)return;
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
  if(r<0&&errno==EINTR)continue;if(r<=0)return -1;done+=(size_t)r;
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
 plain[0]=(unsigned char)(id>>8);plain[1]=(unsigned char)id;memcpy(plain+2,data,n);tx_ch=ch;tx_flags=flags;
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
#define AUDIO_FOCUS_RETRY_US 2000000ULL
#define AUDIO_STOP_IDLE_US 2000000ULL
#define AUDIO_RELEASE_IDLE_US 15000000ULL
static int audio_window(uint64_t announced){return announced<1?1:announced>AUDIO_WINDOW_MAX?AUDIO_WINDOW_MAX:(int)announced;}
static unsigned audio_sent,audio_acks,audio_underflows,audio_stalls,audio_peak,audio_discarded,audio_session,ping_seen;
static int audio_streaming;
static uint64_t audio_full_at,audio_stats_at;
/* audio_focus: 0 none, 1 requested, 2 granted (GAIN, GAIN_TRANSIENT, LOSS_TRANSIENT_CAN_DUCK, GAIN_MEDIA_ONLY). */
static int audio_focus;static uint64_t audio_focus_at,audio_last;
static uint64_t audio_samples,audio_epoch;
static void request_audio_focus(unsigned type,uint64_t now){unsigned char req[]={8,(unsigned char)type};send_msg(0,11,0x12,req,sizeof req);audio_focus_at=now;session_trace("TX audio_focus_request type=%u",type);}
static void audio_on_config(const unsigned char *body,size_t len){
 if(value(body,len,1,0)!=2){session_trace("AUDIO_CONFIG rejected %s",hex16(body,len));return;}
 audio_max_unacked=audio_window(value(body,len,2,1));audio_unacked=0;audio_ready=1;
 fprintf(stderr,"AUDIO_READY 48000 stereo PCM16 window=%d\n",audio_max_unacked);session_trace("AUDIO_READY window=%d",audio_max_unacked);
}
static void audio_start(uint64_t now,uint64_t start){
 unsigned char begin[8]={8};size_t n=1;unsigned id=++audio_session;
 do{begin[n++]=(unsigned char)((id&127)|(id>=128?128:0));id>>=7;}while(id);begin[n++]=16;begin[n++]=0;
 send_msg(audio_channel,11,0x8001,begin,n);audio_streaming=1;audio_unacked=0;audio_epoch=now-start;audio_samples=0;
 session_trace("AUDIO_START session=%u discarded=%u",audio_session,audio_discarded);
}
static void audio_stop(const char *why){
 if(!audio_streaming)return;
 send_msg(audio_channel,11,0x8002,"",0);audio_streaming=0;audio_unacked=0;
 session_trace("AUDIO_STOP session=%u %s sent=%u acks=%u",audio_session,why,audio_sent,audio_acks);
}
static void audio_on_focus(unsigned state){
 audio_focus=state==1||state==2||state==4||state==6?2:0;
 if(!audio_focus)audio_stop("focus_lost");
}
static void audio_on_ack(const unsigned char *body,size_t len){
 int ack=(int)value(body,len,2,1);
 if(!audio_acks)session_trace("RX audio ack first %s unacked=%d",hex16(body,len),audio_unacked);
 audio_acks+=(unsigned)(ack>0?ack:0);audio_unacked=ack>audio_unacked?0:audio_unacked-ack;
}
static void audio_pump(uint64_t now,uint64_t start){
 if(!audio_ready||audio_fd<0)return;
 /* A head unit that stops acknowledging must not mute the stream for good. */
 if(audio_streaming&&audio_unacked>=audio_max_unacked&&now-audio_full_at>2000000){audio_stalls++;session_trace("AUDIO_ACK_STALL unacked=%d window=%d acks=%u",audio_unacked,audio_max_unacked,audio_acks);audio_unacked=0;}
 while(!(audio_streaming&&audio_unacked>=audio_max_unacked)){
  unsigned char pcm[8192],data[8200];ssize_t count=recv(audio_fd,pcm,sizeof pcm,MSG_DONTWAIT);
  if(count<=0)break;
  if(!audio_last){session_trace("AUDIO_PCM_START bytes=%zd focus=%d window=%d",count,audio_focus,audio_max_unacked);audio_stats_at=now;}
  audio_last=now;
  for(ssize_t k=0;k+1<count;k+=2){int sample=(int16_t)(pcm[k]|pcm[k+1]<<8);unsigned level=(unsigned)(sample<0?-sample:sample);if(level>audio_peak)audio_peak=level;}
  if(!audio_streaming){
   if(audio_focus==2)audio_start(now,start);
   else{if(!audio_focus_at||now-audio_focus_at>AUDIO_FOCUS_RETRY_US){request_audio_focus(1,now);audio_focus=1;}audio_discarded++;continue;}
  }
  uint64_t ts=audio_epoch+audio_samples*1000000ULL/48000;audio_samples+=(uint64_t)count/4;
  for(int k=7;k>=0;k--){data[k]=(unsigned char)ts;ts>>=8;}memcpy(data+8,pcm,(size_t)count);
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
  while(j<l){if(field(b,l,&j,&st,&sv,&sb,&sl))return -1;if(st!=5||!sb||value(sb,sl,1,0)!=1)continue;
   size_t k=0,al;unsigned at;uint64_t av;const unsigned char *ab;
   while(k<sl){if(field(sb,sl,&k,&at,&av,&ab,&al))return -1;if(at!=2||!ab)continue;
    unsigned rate=value(ab,al,1,0),bits=value(ab,al,2,0),channels=value(ab,al,3,0);
    if(id>0&&id<256&&bits==16&&channels==1&&rate>=8000&&rate<=48000){mic_rate=rate;fprintf(stderr,"AA_MIC_SELECTED channel=%d rate=%u PCM16 mono\n",id,rate);return id;}
   }
  }
 }fprintf(stderr,"AA_MIC_UNAVAILABLE: no PCM16 mono source\n");return -1;
}
static void microphone_request(void){
 if(!mic_ready||mic_channel<0||mic_open==mic_wanted)return;
 unsigned char request[]={8,(unsigned char)mic_wanted,16,1,24,1,32,4};send_msg(mic_channel,11,0x8005,request,sizeof request);
 mic_open=mic_wanted;fprintf(stderr,"AA_MIC_REQUEST open=%d\n",mic_open);
}
static void microphone_pcm(const unsigned char *body,size_t len,int timestamp){
 size_t skip=timestamp?8:0;if(!mic_open||len<=skip||(len-skip)%2)return;
 const char *path=getenv("AA_VIDEO_SOCKET");if(!path)return;
 struct sockaddr_un target={0};target.sun_family=AF_UNIX;
 if(snprintf(target.sun_path,sizeof target.sun_path,"%s.mic",path)>=(int)sizeof target.sun_path)return;
 int sock=socket(AF_UNIX,SOCK_DGRAM,0);if(sock<0)return;
 unsigned char packet[8196];packet[0]=mic_rate;packet[1]=mic_rate>>8;packet[2]=mic_rate>>16;packet[3]=mic_rate>>24;
 for(size_t at=skip;at<len;){size_t count=len-at;if(count>8192)count=8192;memcpy(packet+4,body+at,count);
  if(sendto(sock,packet,count+4,MSG_DONTWAIT,(struct sockaddr*)&target,sizeof target)<0){if(mic_frames%100==0)perror("AA_MIC_FORWARD");break;}at+=count;
 }close(sock);mic_frames++;if(mic_frames%100==1)fprintf(stderr,"AA_MIC_PCM frames=%lu rate=%u\n",mic_frames,mic_rate);
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
static int find_input(const unsigned char *p,size_t n){
 int fallback=-1;
 size_t i=0,l;unsigned t;uint64_t v;const unsigned char *b;
 while(i<n){if(field(p,n,&i,&t,&v,&b,&l))return -1;if(t!=1||!b)continue;
  int id=-1,has_input=0;size_t j=0,sl;unsigned st;uint64_t sv;const unsigned char *sb;
  while(j<l){if(field(b,l,&j,&st,&sv,&sb,&sl))return -1;if(st==1&&!sb)id=(int)sv;if(st==4&&sb)has_input=1;}
  if(has_input&&id>0&&id<256){if(id==8)return id;if(fallback<0)fallback=id;}
 }return fallback;
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
   const char *video_path=getenv("AA_VIDEO_SOCKET");if(!video_path)return;
   struct sockaddr_un target={0};target.sun_family=AF_UNIX;
   if(snprintf(target.sun_path,sizeof target.sun_path,"%s.touch",video_path)>=(int)sizeof target.sun_path)return;
   int touch_fd=socket(AF_UNIX,SOCK_DGRAM,0);if(touch_fd<0)return;
   if(sendto(touch_fd,report,5,0,(struct sockaddr*)&target,sizeof target)!=5)perror("Touch forward");
   else fprintf(stderr,"AA_TOUCH action=%llu x=%u y=%u\n",(unsigned long long)action,x,y);
   close(touch_fd);return;
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
 const char *suffixes[]={"",".audio",".control"};
 if(snprintf(name,sizeof name,"%s.lock",path)>=(int)sizeof name){errno=ENAMETOOLONG;die("Live lock path");}
 live_lock=open(name,O_CREAT|O_RDWR|O_CLOEXEC|O_NOFOLLOW,0600);
 if(live_lock<0||flock(live_lock,LOCK_EX|LOCK_NB))die("Live sender already running or lock unavailable");
 for(unsigned i=0;i<3;i++){
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
 if(bind(live_listener,(struct sockaddr*)&a,sizeof a)||listen(live_listener,1))die("Video bind/listen");
 fcntl(live_listener,F_SETFL,O_NONBLOCK);
 struct sockaddr_un audio={0};audio.sun_family=AF_UNIX;
 if(snprintf(audio.sun_path,sizeof audio.sun_path,"%s.audio",path)>=(int)sizeof audio.sun_path)exit(1);
 audio_fd=socket(AF_UNIX,SOCK_DGRAM,0);if(audio_fd<0||bind(audio_fd,(struct sockaddr*)&audio,sizeof audio))die("Audio socket");
 fcntl(audio_fd,F_SETFL,O_NONBLOCK);
 struct sockaddr_un control={0};control.sun_family=AF_UNIX;
 if(snprintf(control.sun_path,sizeof control.sun_path,"%s.control",path)>=(int)sizeof control.sun_path)exit(1);
 control_fd=socket(AF_UNIX,SOCK_DGRAM,0);if(control_fd<0||bind(control_fd,(struct sockaddr*)&control,sizeof control))die("Control socket");
 fcntl(control_fd,F_SETFL,O_NONBLOCK);
 fprintf(stderr,"LIVE_VIDEO_READY %s\n",path);
}
static void poll_live_disconnect(void){
  if(live_peer>=0){unsigned char peek;ssize_t n=recv(live_peer,&peek,1,MSG_PEEK|MSG_DONTWAIT);
   if(n==0||(n<0&&errno!=EAGAIN&&errno!=EWOULDBLOCK&&errno!=EINTR)){close(live_peer);live_peer=-1;live_used=0;live_disconnected=1;}
  }
}
static int next_live_frame(size_t *length,uint64_t *stamp){
 if(live_peer<0){live_peer=accept(live_listener,NULL,NULL);if(live_peer<0)return 0;fcntl(live_peer,F_SETFL,O_NONBLOCK);live_used=0;}
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
 for(;i+3<=n;i++)if(!p[i]&&!p[i+1]&&(p[i+2]==1||(i+4<=n&&!p[i+2]&&p[i+3]==1)))return i;return n;
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
static void index_waiting_units(void){
 size_t at=annexb_next(waiting_video,waiting_size,0),begin=0;waiting_units=0;
 while(at<waiting_size&&waiting_units<2){size_t code=waiting_video[at+2]==1?3:4,end=annexb_next(waiting_video,waiting_size,at+code);
  unsigned type=at+code<waiting_size?waiting_video[at+code]&31:0;
  if(type>=1&&type<=5){waiting_unit_off[waiting_units]=begin;waiting_unit_len[waiting_units++]=end-begin;begin=end;}
  at=end;
 }
}
static int send_video(int ch,const unsigned char *data,size_t length,uint64_t stamp){
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
 fragmented_total=(uint32_t)total;
 for(size_t offset=0;offset<total;){size_t chunk=total-offset;if(chunk>15000)chunk=15000;
  tx_ch=ch;tx_flags=8|(offset==0?1:0)|(offset+chunk==total?2:0);
  int r=mbedtls_ssl_write(&ssl,video_message+offset,chunk);check(r,"Video TLS write");
  if(r!=(int)chunk){fprintf(stderr,"Short video write\n");exit(1);}offset+=chunk;
 }
 return 1;
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
  fprintf(stderr,"USB transfer=%u bytes=%zd\n",++transfer,n);
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
static void *usb_fast_reader(void *unused){
 (void)unused;
 sigset_t blocked;sigemptyset(&blocked);sigaddset(&blocked,SIGALRM);sigaddset(&blocked,SIGTERM);sigaddset(&blocked,SIGINT);pthread_sigmask(SIG_BLOCK,&blocked,NULL);
 struct sched_param priority={.sched_priority=20};int rc=pthread_setschedparam(pthread_self(),SCHED_FIFO,&priority);
 fprintf(stderr,"USB_FAST_READER depth=%d realtime=%s\n",FAST_SLOTS,rc?"unavailable":"enabled");
 unsigned produced=0;
 while(!stop){
  if(produced-__atomic_load_n(&fast_read,__ATOMIC_ACQUIRE)==FAST_SLOTS){struct timespec delay={0,100000};nanosleep(&delay,NULL);continue;}
  unsigned slot=produced%FAST_SLOTS;
  ssize_t n=read(reader_usb_fd,fast_data[slot],sizeof fast_data[slot]);
  if(n<0&&errno==EINTR)continue;
  if(n==0){if(getenv("AA_TEST_READER"))break;continue;}
  if(n<0){fprintf(stderr,"USB fast read errno=%d\n",errno);break;}
  fast_lengths[slot]=(size_t)n;
  __atomic_store_n(&fast_write,++produced,__ATOMIC_RELEASE);
 }
 __atomic_store_n(&fast_closed,1,__ATOMIC_RELEASE);return NULL;
}
static void *usb_fast_consumer(void *unused){
 (void)unused;unsigned consumed=0;
 while(!stop){
  if(consumed==__atomic_load_n(&fast_write,__ATOMIC_ACQUIRE)){
   if(__atomic_load_n(&fast_closed,__ATOMIC_ACQUIRE))break;
   struct timespec delay={0,100000};nanosleep(&delay,NULL);continue;
  }
  unsigned slot=consumed%FAST_SLOTS;size_t n=fast_lengths[slot];
  if(raw_capture){fwrite(fast_data[slot],1,n,raw_capture);fflush(raw_capture);}
  size_t offset=0;
  while(offset<n){ssize_t w=write(reader_queue_fd,fast_data[slot]+offset,n-offset);if(w<0&&errno==EINTR)continue;if(w<=0)goto done;offset+=(size_t)w;}
  __atomic_store_n(&fast_read,++consumed,__ATOMIC_RELEASE);
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
int main(int argc,char **argv){
 if(argc==3 && !strcmp(argv[1],"--mtp"))return run_mtp(argv[2]);
 if(argc==2&&!strcmp(argv[1],"--probe-aio")){
  aio_context_t ctx=0;
  if(syscall(SYS_io_setup,8,&ctx)<0){fprintf(stderr,"AIO_UNAVAILABLE errno=%d (%s): kernel support required\n",errno,strerror(errno));return 3;}
  syscall(SYS_io_destroy,ctx);fprintf(stderr,"AIO_AVAILABLE\n");return 0;
 }
 if(argc!=6){fprintf(stderr,"Usage: %s HOST PORT CERT KEY MIRE.h264\n       %s --ffs MOUNT CERT KEY MIRE.h264\n",argv[0],argv[0]);return 2;}
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
 int video_ch=-1,running=0,focused=0,unacked=0,max_unacked=1,frames=0,carplay_seen=0,lost_focus=0;unsigned video_acks=0;uint64_t start=now_us(),next=0;
 /* The Uconnect returns to its native screen when no picture arrives ~5 s after granting focus, and
  * keeps the first codec configuration of a session: the waiting screen is encoded under the
  * iPhone's own SPS/PPS (tooling/waiting_iphone_config.py) and starts at once. */
 uint64_t waiting_delay=getenv("AA_WAITING_DELAY_MS")?strtoull(getenv("AA_WAITING_DELAY_MS"),NULL,10)*1000:0;
 while(!stop&&(getenv("AA_AUTONOMOUS")||now_us()-start<(live_listener>=0?600000000ULL:120000000ULL))){
  poll_live_disconnect();
   if(live_disconnected){
    live_disconnected=0;unacked=0;next=0;carplay_seen=0;need_idr=1;
    unsigned char stale[8192];while(audio_fd>=0&&recv(audio_fd,stale,sizeof stale,MSG_DONTWAIT)>0){}
    audio_stop("carplay_disconnected");
    mic_wanted=0;microphone_request();
    fprintf(stderr,"CARPLAY_DISCONNECTED: returning to waiting screen\n");session_trace("CARPLAY_DISCONNECTED");
   }
  if(control_fd>=0&&video_ch>0){unsigned char event;
   if(recv(control_fd,&event,1,MSG_DONTWAIT)==1){if(event==2||event==3){mic_wanted=event==2;microphone_request();}else if(event==4){if(live_peer>=0)close(live_peer);live_peer=-1;live_used=0;live_disconnected=1;}else if(event==1){unsigned char native[]={0x10,2,0x18,4};send_msg(video_ch,11,0x8007,native,sizeof native);focused=0;fprintf(stderr,"CAR_RETURN: Android Auto native UI requested\n");}}
  }
  audio_pump(now_us(),start);
  /* Without video focus the relay must still be drained, or its 2 s write timeout ends CarPlay video. */
  if(live_listener>=0&&!(running&&focused)){size_t dl;uint64_t ds;unsigned dropped=0;
   while(next_live_frame(&dl,&ds)){live_used=0;dropped++;}
   if(dropped){if(!carplay_seen)session_trace("CARPLAY_FIRST_FRAME unfocused");carplay_seen=1;if(!need_idr)session_trace("VIDEO_DROP unfocused frames=%u",dropped);need_idr=1;}
  }
  if(running&&focused&&unacked<max_unacked&&now_us()>=next){
   size_t length=vn;uint64_t stamp=now_us()-start;
   int live_ready=live_listener>=0?next_live_frame(&length,&stamp):0;
   if(live_ready){if(!carplay_seen)session_trace("CARPLAY_FIRST_FRAME after_waiting=%d",frames>0);carplay_seen=1;}
   if(live_ready&&need_idr){
    if(has_idr(live_buf+12,length)){need_idr=0;session_trace("VIDEO_RESUME_IDR bytes=%zu",length);}
    else{live_used=0;live_ready=0;request_keyframe();}
   }
   if(live_listener<0||live_ready){
    /* One monotonic session clock for every source: CarPlay PTS restart near zero, which the
     * Uconnect renderer treats as late samples after the waiting screen and never displays. */
    int sent=send_video(video_ch,live_listener<0?video:live_buf+12,length,now_us()-start);
    if(live_listener>=0)live_used=0;
    unacked+=sent;frames+=sent;next=now_us()+(live_listener<0?500000:0);
    TRACE("VIDEO frame=%d bytes=%zu unacked=%d source=%s\n",frames,length,unacked,live_listener<0?"test":"CarPlay");
    if(sent&&(frames<=5||!(frames%50)))session_trace("TX video n=%d bytes=%zu unacked=%d src=%s",frames,length,unacked,live_listener<0?"test":"CarPlay");
   }else if(!carplay_seen&&!live_ready&&now_us()-start>=waiting_delay&&waiting_units&&(waiting_units>1||codec_config_pending)){
    /* A single coded picture is resent only with a new codec configuration. */
    int unit=waiting_next;waiting_next=(waiting_next+1)%waiting_units;
    int sent=send_video(video_ch,waiting_video+waiting_unit_off[unit],waiting_unit_len[unit],now_us()-start);
    unacked+=sent;frames+=sent;next=now_us()+1000000;
    TRACE("VIDEO frame=%d bytes=%zu source=Bluetooth-waiting-screen\n",frames,waiting_unit_len[unit]);
    if(sent&&(frames<=5||!(frames%50)))session_trace("TX video n=%d bytes=%zu unacked=%d src=waiting unit=%d",frames,waiting_unit_len[unit],unacked,unit);
   }
  }
  fd_set set;FD_ZERO(&set);FD_SET(fd,&set);struct timeval wait={0,100000};
  if(!mbedtls_ssl_get_bytes_avail(&ssl)&&!left){
   if(!(usb_mode&&usb_pos<usb_len)){
    int max_fd=fd;if(control_fd>=0){FD_SET(control_fd,&set);if(control_fd>max_fd)max_fd=control_fd;}int video_fd=live_peer>=0?live_peer:live_listener;
    if(audio_ready&&audio_unacked<audio_max_unacked&&audio_fd>=0){FD_SET(audio_fd,&set);if(audio_fd>max_fd)max_fd=audio_fd;}
    if(video_fd>=0){FD_SET(video_fd,&set);if(video_fd>max_fd)max_fd=video_fd;}
    if(select(max_fd+1,&set,NULL,NULL,&wait)<=0)continue;
    if(!FD_ISSET(fd,&set))continue;
   }
   int incoming_flags;size_t incoming_len;
   if(receive(&rx_ch,&incoming_flags,incoming,&incoming_len)){session_trace("TRANSPORT_CLOSED");break;}
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
  else if(!(rx_ch==mic_channel&&id<2)&&!(rx_ch==audio_channel&&(id==0x8004||id==0x800b))&&!(rx_ch==input_channel&&id==0x8001)&&!(rx_ch==0&&id==0x0b&&ping_seen++%10))session_trace("RX ch=%d id=%04x len=%zu %s",rx_ch,id,len,hex16(body,len));
  if(rx_ch==0&&id==4){unsigned char name[]={0x22,10,'C','a','r','F','l','u','x','o','i','d'};send_msg(0,11,5,name,sizeof name);}
  else if(rx_ch==0&&id==6){input_channel=find_input(body,len);video_ch=find_video(body,len);audio_channel=live_listener>=0?find_media_audio(body,len):-1;mic_channel=live_listener>=0?find_microphone(body,len):-1;if(live_listener>=0&&selected_codec!=3){fprintf(stderr,"Live CarPlay requires an H264 receiver\n");return 1;}if(video_ch<0){fprintf(stderr,"No H264 video service\n");return 1;}fprintf(stderr,"Selected video service=%d codec=%d AA_DISPLAY=%ux%u\n",video_ch,selected_codec,display_width,display_height);
 load_waiting_video();index_waiting_units();session_trace("SERVICES video=%d input=%d audio=%d mic=%d waiting_units=%d margins=%ux%u",video_ch,input_channel,audio_channel,mic_channel,waiting_units,video_margin_w,video_margin_h);
 for(size_t o=0;o<len;o+=48)session_trace("SDR %zu %s",o,hex64(body+o,len-o<48?len-o:48));
 if(getenv("AA_VIDEO_SOCKET")){char path[4096];snprintf(path,sizeof path,"%s.display",getenv("AA_VIDEO_SOCKET"));FILE *display=fopen(path,"w");if(!display)die("Display negotiation file");fprintf(display,"%u %u %u %u\n",display_width,display_height,video_margin_w,video_margin_h);fclose(display);}if(selected_codec==7){char path[4096];snprintf(path,sizeof path,"%s.hevc",argv[5]);FILE *hf=fopen(path,"rb");if(!hf)die("HEVC test frame");vn=fread(video,1,sizeof video,hf);if(!vn||!feof(hf)){fclose(hf);return 1;}fclose(hf);}open_video(video_ch);if(input_channel>0)open_video(input_channel);if(audio_channel>0)open_video(audio_channel);if(mic_channel>0)open_video(mic_channel);}
  else if(rx_ch==0&&id==11)send_msg(0,11,12,body,len);
  else if(rx_ch==0&&id==0x13)audio_on_focus((unsigned)value(body,len,1,0));
  else if(rx_ch==0&&id==15){send_msg(0,11,16,"",0);break;}
  else if(rx_ch==mic_channel&&id==8){if(value(body,len,1,999)==0){mic_ready=1;fprintf(stderr,"AA_MIC_CHANNEL_READY\n");microphone_request();}else fprintf(stderr,"AA_MIC channel rejected\n");}
  else if(rx_ch==mic_channel&&id==0x8003){if(value(body,len,1,999)==2){mic_ready=1;microphone_request();}else fprintf(stderr,"AA_MIC setup rejected\n");}
  else if(rx_ch==mic_channel&&id==0x8006){mic_session=(int)value(body,len,1,0);if(value(body,len,2,999)!=0){fprintf(stderr,"AA_MIC open rejected\n");mic_open=0;}else fprintf(stderr,"AA_MIC_OPENED session=%d open=%d\n",mic_session,mic_open);}
  else if(rx_ch==mic_channel&&id==0x8001){mic_session=(int)value(body,len,1,1);fprintf(stderr,"AA_MIC_STARTED session=%d\n",mic_session);}
  else if(rx_ch==mic_channel&&(id==0||id==1)){microphone_pcm(body,len,id==0);unsigned char ack[8]={8};size_t an=1;unsigned session=(unsigned)mic_session;do{ack[an++]=(session&127)|(session>=128?128:0);session>>=7;}while(session);ack[an++]=16;ack[an++]=1;send_msg(mic_channel,11,0x8004,ack,an);}
  else if(rx_ch==mic_channel&&id==0x8002){mic_open=0;fprintf(stderr,"AA_MIC_STOPPED\n");}
  else if(rx_ch==audio_channel&&id==8){unsigned char setup[]={8,1};send_msg(audio_channel,11,0x8000,setup,sizeof setup);}
  else if(rx_ch==audio_channel&&id==0x8003)audio_on_config(body,len);
  else if(rx_ch==audio_channel&&id==0x8004)audio_on_ack(body,len);
  else if(rx_ch==audio_channel&&id==0x800b){if(audio_underflows++%25==0)session_trace("RX audio underflow n=%u sent=%u acks=%u",audio_underflows,audio_sent,audio_acks);}
  else if(rx_ch==input_channel&&id==8){if(value(body,len,1,999)!=0){fprintf(stderr,"Input channel rejected\n");return 1;}send_msg(input_channel,11,0x8002,"",0);}
  else if(rx_ch==input_channel&&id==0x8001)forward_touch(body,len);
  else if(rx_ch==video_ch&&id==8){if(value(body,len,1,999)!=0){fprintf(stderr,"Channel open rejected\n");return 1;}unsigned char setup[]={8,(unsigned char)selected_codec};send_msg(video_ch,11,0x8000,setup,sizeof setup);}
  else if(rx_ch==video_ch&&id==0x8003){if(value(body,len,1,0)!=2){fprintf(stderr,"Video not ready\n");return 1;}max_unacked=(int)value(body,len,2,1);if(max_unacked<1||max_unacked>32)max_unacked=1;unsigned char begin[]={8,1,16,(unsigned char)selected_video_config},focus[]={16,1};send_msg(video_ch,11,0x8001,begin,sizeof begin);send_msg(video_ch,11,0x8007,focus,sizeof focus);running=1;}
  else if(rx_ch==video_ch&&id==0x8008){focused=value(body,len,1,0)==1;
   if(!focused){codec_config_pending=1;lost_focus=frames>0;}
   else if(lost_focus){
    /* Back from the native screen the Uconnect waits for a new video SETUP (its focus manager enters
     * WAITING_AA_GAL_VIDEO_SETUP); CONFIG then restarts the stream from a fresh IDR. */
    lost_focus=0;running=0;unacked=0;codec_config_pending=1;need_idr=carplay_seen;
    unsigned char setup[]={8,(unsigned char)selected_codec};send_msg(video_ch,11,0x8000,setup,sizeof setup);session_trace("VIDEO_RESETUP");
    if(need_idr)request_keyframe();
   }
   session_trace("VIDEO_FOCUS %s",focused?"projected":"native");
  }
  else if(rx_ch==video_ch&&id==0x8004){int ack=(int)value(body,len,2,1);unacked=ack>unacked?0:unacked-ack;}
 }
 mic_wanted=0;microphone_request();
 session_trace("SESSION_END frames=%d stop=%d",frames,(int)stop);
 fprintf(stderr,"AA test finished: %d frames emitted\n",frames);close(fd);return frames>0?0:1;
}
