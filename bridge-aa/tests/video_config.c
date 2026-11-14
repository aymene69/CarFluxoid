#define main phone_program_main
#include "../src/aa_phone.c"
#undef main
#include <assert.h>
int main(void){
 char path[100];snprintf(path,sizeof path,"/tmp/aa-video-options-%ld",(long)getpid());setenv("AA_SETTINGS_FILE",path,1);
 unsigned char offer[]={10,18,8,2,26,14,8,3,34,2,8,3,34,2,8,1,34,2,8,2};
 FILE *f=fopen(path,"w");assert(f);fputs("max_resolution=480\n",f);fclose(f);
 assert(find_video(offer,sizeof offer)==2&&selected_video_config==1&&display_width==800&&display_height==480);
 f=fopen(path,"w");fputs("max_resolution=720\n",f);fclose(f);
 assert(find_video(offer,sizeof offer)==2&&selected_video_config==2&&display_width==1280&&display_height==720);
 f=fopen(path,"w");fputs("max_resolution=0\n",f);fclose(f);
 assert(find_video(offer,sizeof offer)==2&&selected_video_config==0&&display_width==1920&&display_height==1080);
 unsigned char only720[]={10,10,8,2,26,6,8,3,34,2,8,2};
 f=fopen(path,"w");fputs("max_resolution=480\n",f);fclose(f);
 assert(find_video(only720,sizeof only720)==2&&selected_video_config==0&&display_height==720);
 unsigned char unknown[]={10,10,8,2,26,6,8,3,34,2,8,99};assert(find_video(unknown,sizeof unknown)==-1);
 unlink(path);puts("Resolution cap, nonzero configuration index, automatic mode and receiver fallback: PASS");
}
