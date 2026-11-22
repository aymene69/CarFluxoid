#include "../kernel/aa_aoa.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>
int main(void){
 assert(aa_aoa_length(0xc0,51,0,0,2)==2);
 assert(aa_aoa_length(0x40,51,0,0,2)<0);
 assert(aa_aoa_length(0xc1,51,0,0,2)<0);
 assert(aa_aoa_length(0xc0,51,0,0,1)<0);
 for(unsigned i=0;i<6;i++)assert(aa_aoa_length(0x40,52,0,i,256)==256);
 assert(aa_aoa_length(0x40,52,0,6,1)<0);
 assert(aa_aoa_length(0x40,52,0,0,257)<0);
 assert(aa_aoa_length(0x40,52,0,0,0)<0);
 assert(aa_aoa_length(0x40,53,0,0,0)==0);
 assert(aa_aoa_length(0x40,53,1,0,0)<0);
 assert(aa_aoa_length(0x40,53,0,1,0)<0);
 assert(aa_aoa_length(0x40,53,0,0,1)<0);
 unsigned char reply[2]={0xff,0xff};
 assert(aa_aoa_protocol_reply(1,reply)==2 && reply[0]==1 && reply[1]==0);
 assert(aa_aoa_protocol_reply(2,reply)==2 && reply[0]==2 && reply[1]==0);
 assert(aa_aoa_protocol_reply(0,reply)<0);
 assert(aa_aoa_protocol_reply(3,reply)<0);
 assert(aa_aoa_length(0x40,58,0,0,0)==0);
 assert(aa_aoa_length(0x40,58,1,0,0)<0);
 assert(aa_aoa_length(0xc0,58,0,0,0)<0);
 assert(aa_aoa_length(0x40,58,0,1,0)<0);
 assert(aa_aoa_length(0x40,58,0,0,1)<0);
 for(unsigned request=54;request<=57;request++)assert(aa_aoa_length(0x40,request,0,0,0)<0);
 assert(!strcmp(AA_ACCESSORY_INTERFACE_NAME,"Android Accessory Interface"));
 puts("AOA request directions, recipient, bounds, start validation and interface name: PASS");
}
