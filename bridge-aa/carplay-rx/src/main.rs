//! Wireless CarPlay receiver for the dongle. No USB gadget or car-side CarPlay.
mod web;
mod microphone;
use std::{sync::Arc,time::{Duration,Instant}};
use async_trait::async_trait;
use catplay_carplay::{carplay_rx::{AirPlayReceiverSink,sink::AirPlayServerShared},
 msg::{Display,InfoMessageResponse},rtsp_frame::{RtspError,RtspResult},
 screen::rx::{ScreenReceiverSink,ScreenReceiverSinkBox},video::{AvccConfigExtended,EncodedVideoFrame}};
use catplay_carplay_rx_gadget::CarPlayWirelessGadget;
use catplay_hap::HomekitStorageFile;
use catplay_mfi::MfiDeviceLocal;
use catplay_util::{AsyncShutdown,EventReconciler,EventSleeper};
use tokio::{io::AsyncWriteExt,net::UnixStream};

struct Audio { socket:std::os::unix::net::UnixDatagram, ring:Option<catplay_carplay::audio::RingConsumer<i16>>, rate:u32, channels:usize, phase:u32, previous:[i16;2], pending:std::collections::VecDeque<Vec<u8>>, dropped:u64 }
impl catplay_carplay::audio::AudioPlayer for Audio {
 type Sample=i16;
 fn init(&mut self,ring:catplay_carplay::audio::RingConsumer<i16>,format:catplay_carplay::audio::AudioStreamBasicDescription)->RtspResult<()>{
  if format.channels_per_frame<1||format.channels_per_frame>2||format.sample_rate<8000{return Err(RtspError::NotSupported);}
  self.rate=format.sample_rate;self.channels=format.channels_per_frame as usize;self.ring=Some(ring);
  log::info!("AUDIO_PCM input_rate={} channels={} output=48000/stereo",self.rate,self.channels);Ok(())
 }
 fn on_event(&mut self,event:catplay_carplay::audio::AudioPlayerEvent){
  use catplay_carplay::audio::{AudioPlayerEvent,RingSegment};
  if !matches!(event,AudioPlayerEvent::AddedDataToRing){return;}
  let Some(ring)=&self.ring else{return;};let mut pcm=Vec::new();
  for segment in ring.iter_segments(16384,false){if let RingSegment::Data(samples)=segment{
   for frame in samples.chunks_exact(self.channels){let current=[frame[0],frame[self.channels-1]];
    self.phase+=48000;while self.phase>=self.rate{self.phase-=self.rate;let f=1.0-self.phase as f64/48000.0;
     for ch in 0..2{let sample=(self.previous[ch] as f64+(current[ch] as f64-self.previous[ch] as f64)*f) as i16;pcm.extend_from_slice(&sample.to_le_bytes());}
    }self.previous=current;
   }
  }}
  for chunk in pcm.chunks(7680){
   if self.pending.len()>=8 {self.pending.pop_front();self.dropped+=1;if self.dropped%64==1{log::warn!("AUDIO_QUEUE backlog bounded: dropped {} stale blocks",self.dropped);}}
   self.pending.push_back(chunk.to_vec());
  }
  while let Some(chunk)=self.pending.front(){
   match self.socket.send(chunk){
    Ok(n) if n==chunk.len()=>{self.pending.pop_front();},
    Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>break,
    Err(e)=>{log::warn!("AUDIO_PCM disconnected: {e}");self.pending.clear();break;},
    Ok(_)=>{self.pending.pop_front();log::warn!("AUDIO_PCM unexpected short datagram");}
   }
  }
 }
}
struct Screen { pipe:UnixStream, start:Instant, config:Option<AvccConfigExtended> }
#[async_trait]
impl ScreenReceiverSink for Screen {
 async fn init(&mut self)->RtspResult<()>{Ok(())}
 async fn set_avcc_config(&mut self,c:AvccConfigExtended)->RtspResult<()>{
  if c.hevc{return Err(RtspError::UnexpectedState("AA bridge currently requires H264".into()));}
  log::info!("CarPlay video configuration {}x{}",c.width,c.height);self.config=Some(c);Ok(())
 }
 async fn process_frame(&mut self,f:EncodedVideoFrame)->RtspResult<()>{
  if let Some(c)=f.config.clone(){self.set_avcc_config(c).await?;}
  let mut bytes=Vec::new();
  if let Some(c)=&self.config{bytes.extend_from_slice(&c.avcc.sps_pps);}
  // ScreenReceiver normally supplies Annex-B. Reject unexpected formats explicitly.
  if !f.data.starts_with(&[0,0,0,1])&&!f.data.starts_with(&[0,0,1]){
   return Err(RtspError::UnexpectedState("Expected Annex-B CarPlay frame".into()));
  }
  bytes.extend_from_slice(&f.data);
  if bytes.len()>1024*1024{return Err(RtspError::UnexpectedState("Video frame exceeds 1 MiB".into()));}
  let stamp=f.pts.0.saturating_duration_since(self.start).as_micros() as u64;
  let mut header=[0u8;12];header[..4].copy_from_slice(&(bytes.len() as u32).to_be_bytes());header[4..].copy_from_slice(&stamp.to_be_bytes());
  tokio::time::timeout(Duration::from_secs(2),async {
   self.pipe.write_all(&header).await?;self.pipe.write_all(&bytes).await
  }).await.map_err(|_|RtspError::Timeout)?.map_err(|_|RtspError::Closed)?;
  log::debug!("CarPlay frame bytes={} pts_us={stamp}",bytes.len());Ok(())
 }
}
impl AsyncShutdown for Screen {}
fn display_size()->(u16,u16) {
 let path=std::env::var("AA_VIDEO_SOCKET").unwrap_or_default()+".display";
 let data=std::fs::read_to_string(path).unwrap_or_default();
 let mut values=data.split_whitespace().filter_map(|v|v.parse::<u16>().ok());
 match (values.next(),values.next()){(Some(w),Some(h)) if w>=320&&h>=240=>(w,h),_=>(800,480)}
}
fn touch_device()->catplay_carplay::msg::HidDevice {
 catplay_carplay::msg::HidDevice {display_uuid:"92EF87D8-88E6-48CD-83A0-7498C886774C".into(),name:"Bridge touchscreen".into(),uuid:"B9B32748-EDAB-494A-A3F1-65A16450C1DE".into(),hid_descriptor:catplay_hid::TouchScreenSingleReport::descriptor(display_size().0-1,display_size().1-1).to_vec().into(),..Default::default()}
}
static TOUCH_GENERATION:std::sync::atomic::AtomicU32=std::sync::atomic::AtomicU32::new(0);
struct Receiver { socket:String, input_task:Option<tokio::task::JoinHandle<()>>, touch_generation:Option<u32>,handle:Option<catplay_carplay::carplay_rx::AirPlayReceiverHandleRef> }
impl Drop for Receiver {fn drop(&mut self){
 if let Some(task)=self.input_task.take(){task.abort();}
 if self.touch_generation.is_some_and(|g|TOUCH_GENERATION.load(std::sync::atomic::Ordering::SeqCst)==g){
  let _=std::fs::remove_file(format!("{}.touch",self.socket));
  if let Ok(s)=std::os::unix::net::UnixDatagram::unbound(){let _=s.set_nonblocking(true);let _=s.send_to(&[4],format!("{}.control",self.socket));}
 }
}}
#[async_trait]
impl AirPlayReceiverSink for Receiver {
 fn init(&mut self,handle:catplay_carplay::carplay_rx::AirPlayReceiverHandleRef)->RtspResult<()>{
  self.handle=Some(handle.clone());
  let path=format!("{}.touch",self.socket);
  // A previous CarPlay session may have left its Unix socket pathname behind.
  let generation=TOUCH_GENERATION.fetch_add(1,std::sync::atomic::Ordering::SeqCst)+1;
  let _=std::fs::remove_file(&path);
  let input=tokio::net::UnixDatagram::bind(&path).map_err(|_|RtspError::Closed)?;
  self.touch_generation=Some(generation);
  self.input_task=Some(tokio::spawn(async move {
   let _guard=path;

   let mut packet=[0u8;64];
   while let Ok(n)=input.recv(&mut packet).await {
    if n!=5||packet[0]>1{continue;}
    let x=u16::from_le_bytes([packet[1],packet[2]]);let y=u16::from_le_bytes([packet[3],packet[4]]);
    let (w,h)=display_size();if x>=w||y>=h{continue;}
    handle.send_hid_report(Instant::now(),&touch_device(),&packet[..5]);
    log::info!("TOUCH sent pressed={} x={x} y={y}",packet[0]);
   }
  }));Ok(())
 }

 async fn on_record(&mut self)->RtspResult<()>{
  let theme=web::options().theme;
  if theme!="auto"{if let Some(handle)=&self.handle{handle.send_command_and_forget(catplay_carplay::msg::Command::SetNightMode(catplay_carplay::msg::CommandSetNightMode{night_mode:theme=="night"}));}}
  Ok(())
 }
 async fn on_command_raw(&mut self,command:&catplay_carplay::msg::Command)->RtspResult<catplay_carplay::rtsp_frame::RtspResponse>{
  if let catplay_carplay::msg::Command::RequestUI(request)=command {
   if request.url.as_deref()!=Some("videoplayback:") {
    let control=std::os::unix::net::UnixDatagram::unbound().map_err(|_|RtspError::Closed)?;
    control.send_to(&[1],format!("{}.control",self.socket)).map_err(|_|RtspError::Closed)?;
    log::info!("CAR_RETURN_REQUESTED url={:?}",request.url);
   }
  }
  Ok(catplay_carplay::rtsp_frame::RtspResponse::new(None,catplay_carplay::rtsp_frame::HttpStatus::Ok))
 }
 async fn on_info(&mut self,info:&mut InfoMessageResponse){
  let options=web::options();
  info.manufacturer="CarFluxoid".into();info.model="CarFluxoid".into();
  info.right_hand_drive=options.right_bar.into();
  info.night_mode=match options.theme.as_str(){"night"=>Some(true.into()),"day"=>Some(false.into()),_=>None};
  let (name,logo,w,h)=web::identity();
  use std::hash::{Hash,Hasher};
  let mut revision=std::collections::hash_map::DefaultHasher::new();
  name.hash(&mut revision);logo.hash(&mut revision);format!("{options:?}").hash(&mut revision);
  info.firmware_revision=format!("CarFluxoid-{:016x}",revision.finish());
  log::info!("CP_SETTINGS name={name:?} logo_bytes={} right_bar={} theme={} fps={} return_button={} touch=true revision={}",logo.len(),options.right_bar,options.theme,options.fps,options.return_button,info.firmware_revision);
  info.oem_icon=Some(logo.clone().into());
  info.oem_icon_label=Some(name);
  info.oem_icon_visible=options.return_button.into();
  info.oem_icons=vec![catplay_carplay::msg::OemIcon{image_data:logo.into(),width_pixels:w,height_pixels:h,prerendered:true.into()}];
  // Retain CatPlay audio capabilities; PCM is forwarded to the AA media sink.
  info.hid_devices.push(touch_device());
  let (w,h)=display_size();let (physical_w,physical_h)=options.physical_size(w,h);log::info!("CP_DISPLAY negotiated {}x{} scale={} physical={}x{}mm",w,h,options.scale_percent,physical_w,physical_h);
  info.displays=vec![Display{width_pixels:w.into(),height_pixels:h.into(),width_physical:physical_w.into(),height_physical:physical_h.into(),max_fps:Some(options.fps.into()),primary_input_device:Some(catplay_carplay::msg::PrimaryInputDevice::TouchScreen),features:if options.assist_buttons{catplay_carplay::msg::DisplayFeature::LOWFI_TOUCH}else{catplay_carplay::msg::DisplayFeature::HIGHFI_TOUCH},uuid:"92EF87D8-88E6-48CD-83A0-7498C886774C".into(),..Default::default()}];
 }
 async fn open_microphone(&mut self,_:catplay_carplay::msg::StreamType,_:catplay_carplay::msg::AudioType,_:catplay_carplay::audio::AudioStreamBasicDescription)->RtspResult<catplay_carplay::audio::AudioRecorderBox<i16>>{
  Ok(Box::new(microphone::Microphone::new(&self.socket)?))
 }
 async fn open_audio(&mut self,_:Duration,_:catplay_carplay::msg::StreamType,_:catplay_carplay::msg::AudioType,_:catplay_carplay::msg::AudioFormat,_:catplay_carplay::audio::AudioStreamBasicDescription,_:bool)->RtspResult<catplay_carplay::audio::AudioPlayerBox<i16>>{
  let socket=std::os::unix::net::UnixDatagram::unbound().map_err(|_|RtspError::Closed)?;
  socket.connect(format!("{}.audio",self.socket)).map_err(|_|RtspError::Closed)?;
  socket.set_nonblocking(true).map_err(|_|RtspError::Closed)?;
  Ok(Box::new(Audio{socket,ring:None,rate:48000,channels:2,phase:0,previous:[0,0],pending:std::collections::VecDeque::new(),dropped:0}))
 }
 async fn open_screen(&mut self,_:Duration)->RtspResult<ScreenReceiverSinkBox>{
  let pipe=UnixStream::connect(&self.socket).await.map_err(|_|RtspError::Closed)?;
  Ok(Box::new(Screen{pipe,start:Instant::now(),config:None}))
 }
}
impl AsyncShutdown for Receiver {}
impl EventSleeper for Receiver {}
impl EventReconciler for Receiver {type Error=RtspError;}
fn firmware_wifi(key:&str)->Option<String>{
 let text=std::fs::read_to_string("/etc/catplay/catplay.conf").ok()?;let mut wifi=false;
 for line in text.lines(){let line=line.trim();if line.starts_with('['){wifi=line=="[wifi_network]";continue;}
  if wifi {if let Some((name,value))=line.split_once('='){if name.trim()==key{return Some(value.trim().trim_matches('"').to_string());}}}
 }None
}
fn main()->Result<(),Box<dyn std::error::Error>>{
 eprintln!("CP_BOOT 1: executable entered");
 if std::env::args().nth(1).as_deref()==Some("--web"){web::serve()?;return Ok(());}
 let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build()?;
 eprintln!("CP_BOOT 2: runtime ready");
 runtime.block_on(run())
}
async fn run()->Result<(),Box<dyn std::error::Error>>{
 eprintln!("CP_BOOT 3: initializing logger");
 env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(if std::env::var("AA_DIAGNOSTIC").as_deref()==Ok("1"){"info"}else{"warn"})).init();
 eprintln!("CP_BOOT 4: logger ready");
 let args:Vec<_>=std::env::args().collect();
 if args.get(1).map(String::as_str)==Some("--self-test"){eprintln!("CP_SELF_TEST_OK");return Ok(());}
 if args.get(1).map(String::as_str)==Some("--tap")&&args.len()==5{
  let x:u16=args[3].parse()?;let y:u16=args[4].parse()?;
  let (w,h)=display_size();if x>=w||y>=h{return Err("Coordinates outside negotiated display".into());}
  let socket=tokio::net::UnixDatagram::unbound()?;socket.connect(&args[2])?;
  socket.send(&catplay_hid::TouchScreenSingleReport::new(true,x,y).to_bytes()).await?;
  tokio::task::yield_now().await;
  socket.send(&catplay_hid::TouchScreenSingleReport::new(false,x,y).to_bytes()).await?;
  return Ok(());
 }
 if args.len()!=4{return Err("Usage: catplay-aa-rx RAM_STATE_DIR WIFI_SSID VIDEO_SOCKET; password in AA_WIFI_PASSWORD".into());}
 let state=std::path::Path::new(&args[1]);
 eprintln!("CP_BOOT 5: loading local MFi identity");
 let mfi=Arc::new(MfiDeviceLocal::new(&std::fs::read(state.join("certificate.p7b"))?,&std::fs::read(state.join("identity.pk8"))?)?);
 eprintln!("CP_BOOT 6: MFi ready");
 // Keep receiver identity and last successful peer across RAM sessions and boots.
 let pairing=std::env::var_os("AA_PAIRING_DIR").map(std::path::PathBuf::from).unwrap_or_else(||"/persist/aa-settings".into());
 match std::fs::create_dir(&pairing){Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>{},Err(e)=>return Err(e.into())};
 use std::os::unix::fs::PermissionsExt;
 std::fs::set_permissions(&pairing,std::fs::Permissions::from_mode(0o700))?;
 let homekit_path=pairing.join("homekit_rx_db.bin");
 if let Err(e)=std::fs::read(&homekit_path){if e.kind()==std::io::ErrorKind::NotFound{if let Ok(bytes)=std::fs::read(state.join("homekit_rx_db.bin")){std::fs::write(&homekit_path,bytes)?;}}else{return Err(e.into());}}
 let homekit=HomekitStorageFile::file_without_metadata(homekit_path.to_str().ok_or("pairing path")?)?;
 let last_peer=pairing.join("last-iphone.txt");
 let password=firmware_wifi("password").or_else(||std::fs::read_to_string("/persist/config.txt").ok().and_then(|text|text.lines().filter_map(|line|line.split_once('=')).filter(|(key,_)|key.trim()=="wifi_password").map(|(_,value)|value.trim().trim_matches('"').to_string()).filter(|value|value.len()>=8&&value.len()<=63).last()).or_else(||std::env::var("AA_WIFI_PASSWORD").ok()));let socket=args[3].clone();
 eprintln!("CP_BOOT 7: starting wireless receiver");
 let mut pairing=catplay_bt::BluezManager::new();
 pairing.register_pin_agent().await?;
 pairing.set_powered("hci0",true).await?;
 pairing.set_pairable("hci0",true).await?;
 pairing.set_discoverable("hci0",true).await?;
 log::info!("Bluetooth pairing agent ready for this temporary bridge test");
 let ssid_override=firmware_wifi("ssid").or_else(||std::fs::read_to_string("/persist/config.txt").ok().and_then(|text|text.lines().filter_map(|line|line.split_once('=')).filter(|(key,_)|key.trim()=="wifi_name").map(|(_,value)|value.trim().trim_matches('"').to_string()).filter(|name|!name.is_empty()&&name.len()<=29).map(|base|format!("{base}_AP")).last()));
 let ssid=ssid_override.as_deref().unwrap_or(&args[2]);
 log::info!("CARFLUXOID_WIFI_SSID {}",ssid);
 let mut receiver=CarPlayWirelessGadget::new(homekit,Some(mfi),AirPlayServerShared::new(),"CarFluxoid","hci0","wlan0",ssid,password.as_deref(),password.is_some(),None,Some(last_peer.to_str().ok_or("peer cache path")?),move||Receiver{socket:socket.clone(),input_task:None,touch_generation:None,handle:None});
 log::info!("Standalone CarPlay receiver starting; no car-side CarPlay dependency");
 loop {
  if let Some(request)=pairing.get_pairing_request(){
   log::info!("Bluetooth pairing request: {request:?}; accepting for bridge test");
   request.accept();
  }
  let _=receiver.reconcile().await;
  tokio::select!{_=tokio::signal::ctrl_c()=>break,_=receiver.sleep()=>{},_=tokio::time::sleep(Duration::from_millis(100))=>{}}
 }
 receiver.shutdown().await;Ok(())
}
#[cfg(test)]mod lifecycle_tests{
 use super::*;
 #[test]fn old_receiver_cannot_remove_new_touch_socket(){
  let base=format!("/tmp/cp-touch-test-{}",std::process::id());let path=format!("{base}.touch");let control_path=format!("{base}.control");
  let control=std::os::unix::net::UnixDatagram::bind(&control_path).unwrap();control.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
  let _socket=std::os::unix::net::UnixDatagram::bind(&path).unwrap();
  TOUCH_GENERATION.store(42,std::sync::atomic::Ordering::SeqCst);
  let old=Receiver{socket:base.clone(),input_task:None,touch_generation:Some(41),handle:None};
  let new=Receiver{socket:base.clone(),input_task:None,touch_generation:Some(42),handle:None};
  drop(old);assert!(std::os::unix::net::UnixDatagram::bind(&path).is_err());
  drop(new);let mut event=[0u8;1];control.recv(&mut event).unwrap();assert_eq!(event,[4]);
  let _replacement=std::os::unix::net::UnixDatagram::bind(&path).unwrap();let _=std::fs::remove_file(path);let _=std::fs::remove_file(control_path);
 }
}
