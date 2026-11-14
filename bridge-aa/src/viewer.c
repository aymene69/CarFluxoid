#include <SDL.h>
#include <unistd.h>
#include <fcntl.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <signal.h>
#include <sys/wait.h>
#include <stdio.h>
int main(void){
 int pipefd[2];if(pipe(pipefd))return 1;
 pid_t decoder=fork();if(decoder<0)return 1;
 if(!decoder){dup2(pipefd[1],1);close(pipefd[0]);close(pipefd[1]);execl("/opt/homebrew/bin/ffmpeg","ffmpeg","-v","error","-probesize","32","-analyzeduration","0","-f","h264","-i","pipe:0","-threads","1","-flags","low_delay","-vf","scale=800:480","-pix_fmt","rgb24","-f","rawvideo","-flush_packets","1","pipe:1",NULL);_exit(127);}
 close(pipefd[1]);fcntl(pipefd[0],F_SETFL,O_NONBLOCK);
 if(SDL_Init(SDL_INIT_VIDEO))return 1;
 SDL_Window *window=SDL_CreateWindow("CarPlay via Android Auto — tactile",SDL_WINDOWPOS_CENTERED,SDL_WINDOWPOS_CENTERED,800,480,SDL_WINDOW_ALLOW_HIGHDPI);
 SDL_Renderer *renderer=SDL_CreateRenderer(window,-1,SDL_RENDERER_ACCELERATED);
 if(!renderer)renderer=SDL_CreateRenderer(window,-1,SDL_RENDERER_SOFTWARE);
 SDL_Texture *texture=SDL_CreateTexture(renderer,SDL_PIXELFORMAT_RGB24,SDL_TEXTUREACCESS_STREAMING,800,480);
 int udp=socket(AF_INET,SOCK_DGRAM,0);struct sockaddr_in target={0};target.sin_family=AF_INET;target.sin_addr.s_addr=htonl(INADDR_LOOPBACK);target.sin_port=htons(atoi(getenv("AA_TOUCH_PORT")?getenv("AA_TOUCH_PORT"):"0"));
 unsigned char *pixels=malloc(800*480*3);size_t used=0;int running=1,pressed=0;
 while(running){SDL_Event event;while(SDL_PollEvent(&event)){
  if(event.type==SDL_QUIT)running=0;
  int action=-1,x=0,y=0;
  if(event.type==SDL_MOUSEBUTTONDOWN&&event.button.button==SDL_BUTTON_LEFT){action=0;pressed=1;x=event.button.x;y=event.button.y;}
  if(event.type==SDL_MOUSEBUTTONUP&&event.button.button==SDL_BUTTON_LEFT){action=1;pressed=0;x=event.button.x;y=event.button.y;}
  if(event.type==SDL_MOUSEMOTION&&pressed){action=2;x=event.motion.x;y=event.motion.y;}
  if(action>=0&&x>=0&&x<800&&y>=0&&y<480){char msg[64];int n=snprintf(msg,sizeof msg,"%d %d %d",action,x,y);sendto(udp,msg,n,0,(struct sockaddr*)&target,sizeof target);}
 }
 ssize_t n=read(pipefd[0],pixels+used,800*480*3-used);
 if(n>0){used+=n;if(used==800*480*3){SDL_UpdateTexture(texture,NULL,pixels,800*3);SDL_RenderClear(renderer);SDL_RenderCopy(renderer,texture,NULL,NULL);SDL_RenderPresent(renderer);used=0;}}
 else if(n==0)break;
 if(n<0)SDL_Delay(1);
 }
 kill(decoder,SIGTERM);waitpid(decoder,NULL,0);free(pixels);close(udp);close(pipefd[0]);SDL_Quit();return 0;
}
