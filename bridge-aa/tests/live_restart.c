#define main phone_program_main
#include "../src/aa_phone.c"
#undef main
#include <assert.h>
#include <sys/wait.h>
int main(void){
 char dir[]="/tmp/aa-restart-XXXXXX";assert(mkdtemp(dir));char path[120];snprintf(path,sizeof path,"%s/video",dir);
 open_live_input(path);
 pid_t pid=fork();assert(pid>=0);
 if(!pid){close(live_lock);open_live_input(path);_exit(99);}
 int status;assert(waitpid(pid,&status,0)==pid);assert(WIFEXITED(status)&&WEXITSTATUS(status)==1);
 /* Simulate process exit: sockets remain on disk, but the owner lock is gone. */
 close(live_listener);close(audio_fd);close(control_fd);close(live_lock);
 open_live_input(path);
 close(live_listener);close(audio_fd);close(control_fd);close(live_lock);
 const char *suffixes[]={"",".audio",".control",".lock"};char name[150];
 for(unsigned i=0;i<4;i++){snprintf(name,sizeof name,"%s%s",path,suffixes[i]);assert(unlink(name)==0);}
 assert(rmdir(dir)==0);puts("Stale live sockets are reclaimed; concurrent sender is rejected: PASS");
}
