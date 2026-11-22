#define main phone_program_main
#include "../src/aa_phone.c"
#undef main
#include <assert.h>

static void expect_report(int receiver,unsigned char type,unsigned char state){
 unsigned char report[2];
 assert(recv(receiver,report,sizeof report,MSG_DONTWAIT)==sizeof report);
 assert(report[0]==type&&report[1]==state);
}

int main(void){
 char bridge[64];
 snprintf(bridge,sizeof bridge,"/tmp/car-controls-%ld",(long)getpid());
 assert(setenv("AA_VIDEO_SOCKET",bridge,1)==0);
 struct sockaddr_un address={.sun_family=AF_UNIX};
 snprintf(address.sun_path,sizeof address.sun_path,"%s.touch",bridge);
 int receiver=socket(AF_UNIX,SOCK_DGRAM,0);
 assert(receiver>=0&&bind(receiver,(struct sockaddr*)&address,sizeof address)==0);

 unsigned char bindings[64];
 /* DHU rejects the whole subscription when offered only a subset of the keys we request.
  * Encode as proto2 unpacked repeated int32 (0x08 <varint>) for full Uconnect VP2 + DHU compatibility. */
 const unsigned char offer[]={10,7,0x55,0x57,0x20,0x80,0x80,0x04,0x58,8,0x06};
 const unsigned char expected[]={8,0x06,8,0x55,8,0x57,8,0x58,8,0x80,0x80,0x04};
 size_t size=encode_input_bindings(bindings,offered_input_keys(offer,sizeof offer));
 assert(size==sizeof expected&&!memcmp(bindings,expected,size));
 assert(encode_input_bindings(bindings,0)==0);

 /* Receive an incoming key press and release in the same protobuf event group. */
 const unsigned char calls[]={34,12,10,4,8,6,16,1,10,4,8,6,16,0};
 forward_car_controls(calls,sizeof calls);
 expect_report(receiver,0x21,3);
 expect_report(receiver,0x21,0);
 const unsigned char next[]={34,6,10,4,8,0x57,16,1};
 forward_car_controls(next,sizeof next);
 expect_report(receiver,0x20,4);
 const unsigned char voice[]={34,6,10,4,8,0x54,16,1};
 forward_car_controls(voice,sizeof voice);
 expect_report(receiver,0x22,1);
 const unsigned char voice_long[]={34,8,10,6,8,0x54,16,1,32,1};
 forward_car_controls(voice_long,sizeof voice_long);
 expect_report(receiver,0x22,2);
 const unsigned char assist[]={34,7,10,5,8,0xdb,0x01,16,1};
 forward_car_controls(assist,sizeof assist);
 expect_report(receiver,0x22,1);
 const unsigned char enter[]={34,6,10,4,8,0x42,16,1};
 forward_car_controls(enter,sizeof enter);
 expect_report(receiver,0x23,1);
 const unsigned char hook[]={34,6,10,4,8,0x4f,16,1};
 forward_car_controls(hook,sizeof hook);
 expect_report(receiver,0x21,1);
 const unsigned char wheel[]={50,17,10,15,8,0x80,0x80,4,16,
                              0xfe,0xff,0xff,0xff,0xff,0xff,0xff,0xff,0xff,1};
 forward_car_controls(wheel,sizeof wheel);
 expect_report(receiver,0x25,0xfe);
 const unsigned char unknown[]={34,6,10,4,8,0x60,16,1};
 forward_car_controls(unknown,sizeof unknown);
 unsigned char pending;
 assert(recv(receiver,&pending,1,MSG_DONTWAIT)==-1&&errno==EAGAIN);
 close(receiver);
 unlink(address.sun_path);
 puts("Car controls: termination, next track, Siri, signed rotary events and unknown key: PASS");
}
