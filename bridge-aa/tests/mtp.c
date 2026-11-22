#include "../src/mtp.h"
#include <assert.h>
#include <stdio.h>
static void command(unsigned char *p,uint16_t op,uint32_t tx,size_t n){memset(p,0,32);mtp_w32(p,n);mtp_w16(p+4,1);mtp_w16(p+6,op);mtp_w32(p+8,tx);}
int main(void){
 unsigned char cmd[32];struct mtp_reply r;uint32_t session=0;
 command(cmd,0x1001,0x12345678,12);assert(mtp_command(cmd,12,&session,&r)==0);
 assert(r.data_size>100 && r.data_size<=sizeof r.data);
 assert(mtp_u32(r.data)==r.data_size && mtp_u16(r.data+4)==2 && mtp_u16(r.data+6)==0x1001);
 assert(mtp_u32(r.data+8)==0x12345678 && mtp_u16(r.data+12)==100 && mtp_u32(r.data+14)==6);
 assert(mtp_u16(r.response+6)==0x2001 && mtp_u32(r.response+8)==0x12345678);
 command(cmd,0x1004,1,12);assert(!mtp_command(cmd,12,&session,&r) && mtp_u16(r.response+6)==0x2003 && !r.data_size);
 command(cmd,0x1002,2,16);assert(!mtp_command(cmd,16,&session,&r) && mtp_u16(r.response+6)==0x201d);
 mtp_w32(cmd+12,7);assert(!mtp_command(cmd,16,&session,&r) && session==7 && mtp_u16(r.response+6)==0x2001);
 assert(!mtp_command(cmd,16,&session,&r) && mtp_u16(r.response+6)==0x201e);
 command(cmd,0x1004,3,12);assert(!mtp_command(cmd,12,&session,&r) && r.data_size==16 && !mtp_u32(r.data+12));
 command(cmd,0x1006,4,24);assert(!mtp_command(cmd,24,&session,&r) && r.response_size==16 && !mtp_u32(r.response+12));
 command(cmd,0x1007,5,24);assert(!mtp_command(cmd,24,&session,&r) && r.data_size==16 && !mtp_u32(r.data+12));
 command(cmd,0x9999,6,12);assert(!mtp_command(cmd,12,&session,&r) && mtp_u16(r.response+6)==0x2005 && !r.data_size);
 command(cmd,0x1003,7,12);assert(!mtp_command(cmd,12,&session,&r) && !session);
 for(size_t n=0;n<12;n++)assert(mtp_command(cmd,n,&session,&r)<0);
 command(cmd,0x1001,1,13);assert(mtp_command(cmd,13,&session,&r)<0);
 command(cmd,0x1001,1,12);mtp_w16(cmd+4,2);assert(mtp_command(cmd,12,&session,&r)<0);
 command(cmd,0x1001,1,12);assert(mtp_command(cmd,32,&session,&r)<0);
 puts("MTP DeviceInfo, empty storage, session state, transaction IDs and malformed containers: PASS");
}
