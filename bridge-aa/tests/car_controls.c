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
 size_t size=encode_input_bindings(bindings);
 assert(bindings[0]==10&&bindings[1]==size-2);
 assert(!memcmp(bindings+size-3,"\x80\x80\x04",3));

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
