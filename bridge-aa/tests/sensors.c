#define main phone_program_main
#include "../src/aa_phone.c"
#undef main
#include <assert.h>

static int32_t get_le32(const unsigned char *p){
 return (int32_t)((uint32_t)p[0]|((uint32_t)p[1]<<8)|((uint32_t)p[2]<<16)|((uint32_t)p[3]<<24));
}

static size_t put_varint(unsigned char *dst,uint64_t v){
 size_t n=0;
 while(v>=128){dst[n++]=(unsigned char)((v&127)|128);v>>=7;}
 dst[n++]=(unsigned char)v;
 return n;
}

int main(void){
 char bridge[64],night_path[80];
 snprintf(bridge,sizeof bridge,"/tmp/sensors-%ld",(long)getpid());
 snprintf(night_path,sizeof night_path,"%s.night",bridge);
 assert(setenv("AA_VIDEO_SOCKET",bridge,1)==0);
 struct sockaddr_un address={.sun_family=AF_UNIX};
 snprintf(address.sun_path,sizeof address.sun_path,"%s.touch",bridge);
 int receiver=socket(AF_UNIX,SOCK_DGRAM,0);
 assert(receiver>=0&&bind(receiver,(struct sockaddr*)&address,sizeof address)==0);

 /* Uconnect VP2 SensorSourceService on channel 7: sensors 1, 3, 10, 13, 20, 21. */
 const unsigned char sdr[]={
  0x0a,28,
   0x08,7,
   0x12,24,
    0x0a,2,0x08,1,
    0x0a,2,0x08,3,
    0x0a,2,0x08,10,
    0x0a,2,0x08,13,
    0x0a,2,0x08,20,
    0x0a,2,0x08,21,
 };
 uint32_t mask=0;
 assert(find_sensor(sdr,sizeof sdr,&mask)==7);
 assert((mask&(1u<<1))&&(mask&(1u<<3))&&(mask&(1u<<10))&&(mask&(1u<<20)));

 unsigned char req[16];
 size_t rlen=encode_sensor_request(req,10,0);
 assert(rlen==4&&req[0]==0x08&&req[1]==10&&req[2]==0x10&&req[3]==0);
 rlen=encode_sensor_request(req,1,1000);
 assert(rlen==5&&req[0]==0x08&&req[1]==1&&req[2]==0x10&&req[3]==0xe8&&req[4]==0x07);

 /* NightModeData (field 10 = tag 0x52): night=1, duplicate night=1 (dedup), then night=0. */
 const unsigned char night_on[]={0x52,2,0x08,1};
 parse_sensor_batch(night_on,sizeof night_on);
 parse_sensor_batch(night_on,sizeof night_on);
 unsigned char pkt[64];
 assert(recv(receiver,pkt,sizeof pkt,MSG_DONTWAIT)==2);
 assert(pkt[0]==0x26&&pkt[1]==1);
 assert(recv(receiver,pkt,sizeof pkt,MSG_DONTWAIT)==-1&&errno==EAGAIN);
 FILE *nf=fopen(night_path,"r");assert(nf);
 int nval=-1;assert(fscanf(nf,"%d",&nval)==1&&nval==1);fclose(nf);

 const unsigned char night_off[]={0x52,2,0x08,0};
 parse_sensor_batch(night_off,sizeof night_off);
 assert(recv(receiver,pkt,sizeof pkt,MSG_DONTWAIT)==2);
 assert(pkt[0]==0x26&&pkt[1]==0);

 /* LocationData (field 1 = tag 0x0a) with positive lat and negative signed int32 lon. */
 unsigned char loc[64];size_t ln=0;
 loc[ln++]=0x10;ln+=put_varint(loc+ln,(uint64_t)(int64_t)488566000);       /* field 2: lat_e7 */
 loc[ln++]=0x18;ln+=put_varint(loc+ln,(uint64_t)(int64_t)-23522000);       /* field 3: lon_e7 */
 loc[ln++]=0x20;ln+=put_varint(loc+ln,4500);                               /* field 4: acc_e3 */
 loc[ln++]=0x28;ln+=put_varint(loc+ln,(uint64_t)(int64_t)3550);            /* field 5: alt_e2 */
 loc[ln++]=0x30;ln+=put_varint(loc+ln,12500);                              /* field 6: speed_e3 */
 loc[ln++]=0x38;ln+=put_varint(loc+ln,90000000);                           /* field 7: bearing_e6 */
 unsigned char batch[80];size_t bn=0;
 batch[bn++]=0x0a;batch[bn++]=(unsigned char)ln;memcpy(batch+bn,loc,ln);bn+=ln;
 parse_sensor_batch(batch,bn);
 assert(recv(receiver,pkt,sizeof pkt,MSG_DONTWAIT)==29);
 assert(pkt[0]==0x27);
 assert(get_le32(pkt+1)==488566000);
 assert(get_le32(pkt+5)==-23522000);
 assert((uint32_t)get_le32(pkt+9)==4500u);
 assert(get_le32(pkt+13)==3550);
 assert(get_le32(pkt+17)==12500);
 assert(get_le32(pkt+21)==90000000);
 assert((uint32_t)get_le32(pkt+25)==0x0fu);

 /* Null Island (0, 0) must be ignored. */
 const unsigned char null_loc[]={0x0a,4,0x10,0,0x18,0};
 parse_sensor_batch(null_loc,sizeof null_loc);
 assert(recv(receiver,pkt,sizeof pkt,MSG_DONTWAIT)==-1&&errno==EAGAIN);

 /* SpeedData (field 3 = tag 0x1a): speed_e3 = 13889 (50 km/h). */
 unsigned char spd[16];size_t sn=0;
 spd[sn++]=0x08;sn+=put_varint(spd+sn,13889);
 unsigned char sbatch[24];sbatch[0]=0x1a;sbatch[1]=(unsigned char)sn;memcpy(sbatch+2,spd,sn);
 parse_sensor_batch(sbatch,sn+2);
 assert(recv(receiver,pkt,sizeof pkt,MSG_DONTWAIT)==6);
 assert(pkt[0]==0x28&&get_le32(pkt+1)==13889);

 /* Uconnect VP2 SDR ch=5 (Guidance 16000/16/mono) + ch=9 (BluetoothService). */
 const unsigned char uconnect_sdr[]={
  0x0a,0x11, 0x08,0x05, 0x1a,0x0d, 0x08,0x01, 0x10,0x01, 0x1a,0x07, 0x08,0x80,0x7d, 0x10,0x10, 0x18,0x01,
  0x0a,0x1a, 0x08,0x09, 0x32,0x16, 0x0a,0x11,
   '0','2',':','1','a',':','2','b',':','3','c',':','4','d',':','5','e',
   0x12,0x01,0x02,
 };
 assert(find_guidance_audio(uconnect_sdr,sizeof uconnect_sdr)==5);
 assert(guidance_rate==16000&&guidance_channels==1);
 char mac[18]={0};
 assert(find_bluetooth_mac(uconnect_sdr,sizeof uconnect_sdr,mac)==1);
 assert(strcmp(mac,"02:1A:2B:3C:4D:5E")==0);

 /* Guidance focus state 7 (GAIN_TRANSIENT_GUIDANCE_ONLY) grants guidance without claiming media. */
 audio_focus=0;guidance_focus=0;
 audio_on_focus(7);
 assert(guidance_focus==2&&audio_focus==0);
 audio_on_focus(3);
 assert(guidance_focus==0&&audio_focus==0);

 /* NowPlaying 0x30 datagram -> MediaPlaybackMetadata protobuf (fields 1,2,3,6). */
 const unsigned char np[]={
  0x30, 2, 245, 0,
  9,'O','n','e',' ','M','o','r','e','!',
  9,'D','a','f','t',' ','P','u','n','k',
  9,'D','i','s','c','o','v','e','r','y',
  7,'S','p','o','t','i','f','y',
 };
 unsigned char meta[128];unsigned st=0;char app[64]={0};
 size_t mlen=encode_media_metadata(meta,sizeof meta,np,sizeof np,&st,app);
 assert(mlen>0&&st==2&&strcmp(app,"Spotify")==0);
 assert(value(meta,mlen,6,0)==245);
 size_t mi=0,ml;unsigned mt;uint64_t mv;const unsigned char *mb;
 int saw_song=0,saw_artist=0,saw_album=0;
 while(mi<mlen&&!field(meta,mlen,&mi,&mt,&mv,&mb,&ml)){
  if(mt==1&&mb&&ml==9&&!memcmp(mb,"One More!",9))saw_song=1;
  if(mt==2&&mb&&ml==9&&!memcmp(mb,"Daft Punk",9))saw_artist=1;
  if(mt==3&&mb&&ml==9&&!memcmp(mb,"Discovery",9))saw_album=1;
 }
 assert(saw_song&&saw_artist&&saw_album);

 close(receiver);
 unlink(address.sun_path);
 unlink(night_path);
 puts("Sensors, guidance audio, Bluetooth MAC and NowPlaying metadata: PASS");
 return 0;
}
