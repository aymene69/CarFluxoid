#define main phone_program_main
#include "../src/aa_phone.c"
#undef main
#include <assert.h>
int main(void){
 size_t at=0;uint64_t v=0;
 unsigned char good[]={0xac,2};assert(varint(good,sizeof good,&at,&v)==0&&v==300);
 unsigned char truncated[]={0x80};at=0;assert(varint(truncated,1,&at,&v)<0);
 unsigned char overflow[]={255,255,255,255,255,255,255,255,255,255};at=0;assert(varint(overflow,sizeof overflow,&at,&v)<0);
 unsigned char discovery[]={10,6,8,3,26,2,8,3};assert(find_video(discovery,sizeof discovery)==3);
 unsigned char hevc[]={10,6,8,3,26,2,8,7};assert(find_video(hevc,sizeof hevc)==3&&selected_codec==7);
 unsigned char invalid[]={10,255,255,255,255,15};assert(find_video(invalid,sizeof invalid)==-1);
 unsigned char audio[]={10,6,8,4,26,2,8,1};assert(find_video(audio,sizeof audio)==-1);
 unsigned char inputs[]={10,4,8,20,34,0,10,4,8,8,34,0};uint64_t keys;
 assert(find_input(inputs,sizeof inputs,&keys)==8&&keys==0);
 puts("Parser bounds, varints and service selection: PASS");return 0;
}
