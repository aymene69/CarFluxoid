#include "../kernel/aa_aoa.h"
#include <assert.h>
#include <stdio.h>
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
 puts("AOA request directions, recipient, bounds and start validation: PASS");
}
