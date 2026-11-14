#define main phone_program_main
#include "../src/aa_phone.c"
#undef main
#include <assert.h>
int main(void){
 unsigned char offer[]={10,17,8,9,42,13,8,1,18,9,8,128,125,16,16,24,1};
 /* Nested sizes: service=15, source=11, configuration=7. */
 offer[1]=15;offer[5]=11;offer[9]=7;
 assert(find_microphone(offer,sizeof offer)==9&&mic_rate==16000);
 offer[sizeof offer-1]=2;assert(find_microphone(offer,sizeof offer)==-1);offer[sizeof offer-1]=1;
 assert(find_microphone(offer,sizeof offer-1)==-1);
 char path[100];snprintf(path,sizeof path,"/tmp/aa-mic-test-%ld",(long)getpid());setenv("AA_VIDEO_SOCKET",path,1);
 struct sockaddr_un a={0};a.sun_family=AF_UNIX;snprintf(a.sun_path,sizeof a.sun_path,"%s.mic",path);
 int sock=socket(AF_UNIX,SOCK_DGRAM,0);assert(sock>=0&&bind(sock,(struct sockaddr*)&a,sizeof a)==0);
 struct timeval timeout={0,100000};setsockopt(sock,SOL_SOCKET,SO_RCVTIMEO,&timeout,sizeof timeout);
 unsigned char body[]={0,0,0,0,0,0,0,1,0x12,0x34,0x56,0x78},packet[32];
 mic_rate=16000;mic_open=1;microphone_pcm(body,sizeof body,1);assert(recv(sock,packet,sizeof packet,0)==8);assert(packet[0]==0x80&&packet[1]==0x3e&&!memcmp(packet+4,body+8,4));
 mic_open=0;microphone_pcm(body,sizeof body,1);assert(recv(sock,packet,sizeof packet,0)<0);
 mic_open=1;microphone_pcm(body,9,1);assert(recv(sock,packet,sizeof packet,0)<0);
 int peers[2];assert(socketpair(AF_UNIX,SOCK_STREAM,0,peers)==0);live_peer=peers[0];live_disconnected=0;live_used=4;
 poll_live_disconnect();assert(live_peer>=0&&!live_disconnected);
 close(peers[1]);poll_live_disconnect();assert(live_peer==-1&&live_disconnected&&live_used==0);
 close(sock);unlink(a.sun_path);puts("Microphone discovery, format validation, timestamp removal, PCM forwarding and closed-stream gating: PASS");
}
