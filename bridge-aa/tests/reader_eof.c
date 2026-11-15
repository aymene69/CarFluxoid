#define main phone_program_main
#include "../src/aa_phone.c"
#undef main
#include <assert.h>
int main(void){
 int pair[2];assert(socketpair(AF_UNIX,SOCK_STREAM,0,pair)==0);
 fd=pair[0];close(pair[1]);usb_mode=1;buffered_test=0;usb_pos=usb_len=0;stop=0;
 /* A closed reader pipe must terminate even outside buffered-test mode. */
 alarm(2);unsigned char byte;assert(all(0,&byte,1)==-1);alarm(0);close(fd);
 puts("USB reader EOF terminates instead of spinning: PASS");return 0;
}
