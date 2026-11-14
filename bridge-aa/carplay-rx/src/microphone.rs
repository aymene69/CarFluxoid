//! Android Auto PCM microphone source, enabled only while CarPlay requests it.
use std::{sync::{Arc,atomic::{AtomicBool,Ordering}},os::unix::net::UnixDatagram};
use catplay_carplay::{audio::{AudioRecorder,AudioRecorderControl,AudioRecorderEvent,AudioStreamBasicDescription,RingProducer},rtsp_frame::{RtspError,RtspResult}};

pub struct Microphone {
 path:String,control_path:String,socket:Option<tokio::net::UnixDatagram>,
 active:Arc<AtomicBool>,control:Option<AudioRecorderControl>,task:Option<tokio::task::JoinHandle<()>>,
}
impl Microphone {
 pub fn new(video:&str)->RtspResult<Self>{
  let path=format!("{video}.mic");
  let socket=tokio::net::UnixDatagram::bind(&path).map_err(|_|RtspError::Closed)?;
  Ok(Self{path,control_path:format!("{video}.control"),socket:Some(socket),active:Arc::new(AtomicBool::new(false)),control:None,task:None})
 }
 fn request(&self,open:bool){if let Ok(s)=UnixDatagram::unbound(){let _=s.set_nonblocking(true);if let Err(e)=s.send_to(&[if open{2}else{3}],&self.control_path){log::warn!("CP_MIC_CONTROL {e}");}}}
}
/// Continuous mono rate conversion; preserves phase across incoming AA packets.
#[derive(Default)]
struct Resampler{phase:u32,previous:i16,rate:u32}
impl Resampler{
 fn convert(&mut self,pcm:&[u8],rate:u32,out_rate:u32,channels:usize)->Vec<i16>{
  if self.rate!=rate{self.phase=0;self.previous=0;self.rate=rate;}
  let mut output=Vec::new();
  for b in pcm.chunks_exact(2){let sample=i16::from_le_bytes([b[0],b[1]]);self.phase+=out_rate;
   while self.phase>=rate{self.phase-=rate;let f=1.0-self.phase as f64/out_rate as f64;
    let value=(self.previous as f64+(sample as f64-self.previous as f64)*f) as i16;
    for _ in 0..channels{output.push(value);}
   }self.previous=sample;
  }output
 }
}
impl AudioRecorder for Microphone {
 type Sample=i16;
 fn init(&mut self,mut ring:RingProducer<i16>,format:AudioStreamBasicDescription,control:AudioRecorderControl)->RtspResult<()>{
  if !(8000..=48000).contains(&format.sample_rate)||!(1..=2).contains(&format.channels_per_frame){return Err(RtspError::NotSupported);}
  let socket=self.socket.take().ok_or(RtspError::Closed)?;let active=self.active.clone();self.control=Some(control.clone());
  self.task=Some(tokio::spawn(async move{
   let mut packet=[0u8;8196];let mut resampler=Resampler::default();let mut blocks=0u64;
   loop{let n=match socket.recv(&mut packet).await{Ok(n)=>n,Err(e)=>{log::warn!("CP_MIC_RECEIVE {e}");break;}};
    if !active.load(Ordering::Acquire){resampler=Resampler::default();continue;}
    if n<=4||(n-4)%2!=0{continue;}let rate=u32::from_le_bytes(packet[..4].try_into().unwrap());if !(8000..=48000).contains(&rate){continue;}
    let pcm=resampler.convert(&packet[4..n],rate,format.sample_rate,format.channels_per_frame as usize);
    ring.write(&pcm);control.added_data_to_ring();control.start();blocks+=1;
    if blocks%100==1{log::info!("CP_MIC_PCM blocks={blocks} input={rate} output={} channels={}",format.sample_rate,format.channels_per_frame);}
   }
  }));log::info!("CP_MIC_READY rate={} channels={}",format.sample_rate,format.channels_per_frame);Ok(())
 }
 fn on_event(&mut self,event:AudioRecorderEvent){match event{
  AudioRecorderEvent::Start=>{self.active.store(true,Ordering::Release);self.request(true);},
  AudioRecorderEvent::Pause|AudioRecorderEvent::Stop=>{self.active.store(false,Ordering::Release);if let Some(c)=&self.control{c.pause();}self.request(false);},
  AudioRecorderEvent::DrainedDataFromRing=>{}
 }}
}
impl Drop for Microphone{fn drop(&mut self){self.active.store(false,Ordering::Release);self.request(false);if let Some(c)=&self.control{c.pause();}if let Some(t)=self.task.take(){t.abort();}let _=std::fs::remove_file(&self.path);}}
#[cfg(test)]mod tests{use super::*;
 #[test]fn rate_and_packet_boundaries(){let bytes:Vec<u8>=(0..1600i16).flat_map(i16::to_le_bytes).collect();let mut a=Resampler::default();let full=a.convert(&bytes,16000,24000,1);assert_eq!(full.len(),2400);let mut b=Resampler::default();let split:Vec<_>=bytes.chunks(318).flat_map(|p|b.convert(p,16000,24000,1)).collect();assert_eq!(full,split);}
 #[test]fn mono_to_stereo_and_downsample(){let pcm=vec![0u8;960*2];assert_eq!(Resampler::default().convert(&pcm,48000,16000,2).len(),640);}
}
#[cfg(test)]mod lifecycle_tests{use super::*;use catplay_carplay::audio::{RingBuffer,RingSegment};
 #[tokio::test]async fn capture_is_gated_and_stops_on_drop(){
  let base=format!("/tmp/cp-mic-test-{}",std::process::id());let command_path=format!("{base}.control");let command=tokio::net::UnixDatagram::bind(&command_path).unwrap();
  let mut mic=Microphone::new(&base).unwrap();let writer=tokio::net::UnixDatagram::unbound().unwrap();writer.connect(&mic.path).unwrap();
  let (ring,mut received)=RingBuffer::<i16>::spsc(8192);let control=AudioRecorderControl::default();
  mic.init(ring,AudioStreamBasicDescription::fill_pcm(16000,16,16,1,false),control.clone()).unwrap();
  let mut packet=16000u32.to_le_bytes().to_vec();packet.extend_from_slice(&1234i16.to_le_bytes());
  writer.send(&packet).await.unwrap();tokio::time::sleep(std::time::Duration::from_millis(10)).await;assert!(!control.is_running());
  mic.on_event(AudioRecorderEvent::Start);let mut b=[0;1];command.recv(&mut b).await.unwrap();assert_eq!(b,[2]);
  writer.send(&packet).await.unwrap();tokio::time::sleep(std::time::Duration::from_millis(10)).await;assert!(control.is_running());
  let samples:Vec<i16>=received.iter_segments(100,false).filter_map(|s|if let RingSegment::Data(v)=s{Some(v.to_vec())}else{None}).flatten().collect();assert_eq!(samples,vec![1234]);
  mic.on_event(AudioRecorderEvent::Pause);command.recv(&mut b).await.unwrap();assert_eq!(b,[3]);assert!(!control.is_running());
  drop(mic);command.recv(&mut b).await.unwrap();assert_eq!(b,[3]);assert!(!std::path::Path::new(&format!("{base}.mic")).exists());let _=std::fs::remove_file(command_path);
 }
}
