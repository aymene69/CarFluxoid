#define main aa_application_main
#include "../src/aa_phone.c"
#undef main
#define TEST_BYTES (3*1024*1024)
static int test_writer;
static void *feed(void *unused){
 (void)unused;unsigned char data[37];size_t done=0;
 while(done<TEST_BYTES){size_t n=TEST_BYTES-done;if(n>sizeof data)n=sizeof data;for(size_t i=0;i<n;i++)data[i]=(unsigned char)((done+i)%251);size_t off=0;while(off<n){ssize_t w=write(test_writer,data+off,n-off);if(w<=0)exit(4);off+=(size_t)w;}done+=n;}
 close(test_writer);return NULL;
}
int main(void){
 setenv("AA_USB_FAST_READ","1",1);setenv("AA_TEST_READER","1",1);
 int input[2];if(socketpair(AF_UNIX,SOCK_STREAM,0,input))return 2;
 test_writer=input[0];fd=input[1];start_usb_reader();pthread_t writer;pthread_create(&writer,NULL,feed,NULL);
 unsigned char data[4096];size_t done=0;ssize_t n;
 while((n=read(fd,data,sizeof data))>0){for(ssize_t i=0;i<n;i++)if(data[i]!=(unsigned char)((done+i)%251))return 5;done+=(size_t)n;}
 pthread_join(writer,NULL);printf("FAST_READER ordered byte test: %zu bytes\n",done);return done==TEST_BYTES?0:6;
}
