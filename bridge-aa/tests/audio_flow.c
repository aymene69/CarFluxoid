#define main phone_program_main
#include "../src/aa_phone.c"
#undef main
#include <assert.h>
/* Fake head unit: every message the sender emits is captured instead of going through TLS. */
static int out_n,out_ch[64],out_id[64];static size_t out_len[64];static unsigned char out_body[64][16];
static void capture(int ch,int id,const void *data,size_t n){
 assert(out_n<64);out_ch[out_n]=ch;out_id[out_n]=id;out_len[out_n]=n;memcpy(out_body[out_n],data,n<16?n:16);out_n++;
}
static int pcm_peer;static unsigned char pcm[3840];
static void play(int blocks){for(int i=0;i<blocks;i++)assert(send(pcm_peer,pcm,sizeof pcm,0)==(ssize_t)sizeof pcm);}
int main(void){
 send_hook=capture;audio_channel=4;
 int pair[2];assert(!socketpair(AF_UNIX,SOCK_DGRAM,0,pair));audio_fd=pair[0];pcm_peer=pair[1];
 for(size_t i=0;i<sizeof pcm;i+=2){pcm[i]=0x10;pcm[i+1]=0x27;} /* level 10000 */
 const uint64_t start=0,t=10000000;
 /* Uconnect VP2 media sink CONFIG from the car: HEADUNIT, max_unacked 4096, index 0. */
 unsigned char uconnect[]={0x08,0x02,0x10,0x80,0x20,0x18,0x00};
 audio_on_config(uconnect,sizeof uconnect);
 assert(audio_ready&&audio_max_unacked==4096&&out_n==0); /* no START before audio focus */
 /* CarPlay PCM without focus: request GAIN once, stream nothing. */
 play(1);audio_pump(t,start);
 assert(out_n==1&&out_ch[0]==0&&out_id[0]==0x12&&out_len[0]==2&&out_body[0][1]==1&&audio_discarded==1&&!audio_streaming);
 play(1);audio_pump(t+100000,start);
 assert(out_n==1&&audio_discarded==2); /* request pending: no repeat */
 /* GAIN -> START session 1, configuration 0, then timestamped PCM. */
 audio_preroll_us=60000;audio_on_focus(1);play(1);audio_pump(t+140000,start);
 assert(out_n==1&&!audio_streaming);
 audio_pump(t+199999,start);assert(out_n==1);
 audio_pump(t+200000,start);audio_preroll_us=0;
 assert(out_n==3&&out_ch[1]==4&&out_id[1]==0x8001&&out_len[1]==4&&!memcmp(out_body[1],"\x08\x01\x10\x00",4));
 assert(out_ch[2]==4&&out_id[2]==0&&out_len[2]==sizeof pcm+8&&audio_streaming&&audio_sent==1&&audio_peak==10000);
 /* The announced window lets blocks flow ahead of acknowledgements. */
 play(5);audio_pump(t+300000,start);assert(audio_sent==6&&audio_unacked==6);
 uint64_t stamp=0;
 for(int i=0;i<8;i++)stamp=(stamp<<8)|out_body[3][i];
 assert(stamp==t+300000); /* a delayed burst starts at current session time */
 unsigned char ack[]={0x08,0x01,0x10,0x01};audio_on_ack(ack,sizeof ack);assert(audio_unacked==5&&audio_acks==1);
 /* Quiet iPhone: STOP after 2 s, focus kept for a quick restart. */
 audio_pump(t+300000+AUDIO_STOP_IDLE_US+1,start);
 assert(out_id[out_n-1]==0x8002&&out_len[out_n-1]==0&&!audio_streaming&&audio_focus==2);
 /* Next song: new session id. */
 int before=out_n;play(1);audio_pump(t+3000000,start);
 assert(out_n==before+2&&out_id[before]==0x8001&&out_body[before][1]==2&&out_id[before+1]==0);
 /* Focus loss while playing: STOP; re-request only after the retry delay. */
 audio_on_focus(3);assert(out_id[out_n-1]==0x8002&&!audio_streaming&&!audio_focus);
 before=out_n;unsigned dropped=audio_discarded;play(1);audio_pump(t+3100000,start);
 assert(out_n==before+1&&out_id[before]==0x12&&out_body[before][1]==1&&audio_discarded==dropped+1);
 /* Silence for 15 s: focus released. */
 audio_on_focus(1);audio_pump(t+30000000,start);
 assert(out_id[out_n-1]==0x12&&out_body[out_n-1][1]==4&&!audio_focus&&!audio_last);
 assert(audio_window(0)==1&&audio_window(100000)==AUDIO_WINDOW_MAX);
 puts("Audio flow: CONFIG window 4096, focus before START, new session per stream, STOP on silence and focus loss, release after 15 s: PASS");
}
