//! Wireless CarPlay receiver for the dongle. No USB gadget or car-side CarPlay.
mod web;
mod microphone;
mod gps_iap2;
use std::{io::{Error,ErrorKind,IoSlice},sync::Arc,time::{Duration,Instant}};
use async_trait::async_trait;
use catplay_carplay::{carplay_rx::{AirPlayReceiverSink,sink::AirPlayServerShared},
 msg::{Display,InfoMessageResponse},rtsp_frame::{RtspError,RtspResult},
 screen::rx::{ScreenReceiverSink,ScreenReceiverSinkBox},video::{AvccConfigExtended,EncodedVideoFrame}};
use catplay_carplay_rx_gadget::CarPlayWirelessGadget;
use catplay_hap::HomekitStorageFile;
use catplay_mfi::MfiDeviceLocal;
use catplay_util::{AsyncShutdown,EventReconciler,EventSleeper};
use tokio::{io::AsyncWriteExt,net::UnixStream};

struct Audio { socket:std::os::unix::net::UnixDatagram, ring:Option<catplay_carplay::audio::RingConsumer<i16>>, rate:u32, channels:usize, out_rate:u32, out_channels:usize, phase:u32, previous:[i16;2], pending:std::collections::VecDeque<Vec<u8>>, pcm_buf:Vec<u8>, dropped:u64, last_pcm:Option<Instant> }
impl catplay_carplay::audio::AudioPlayer for Audio {
 type Sample=i16;
 fn init(&mut self,ring:catplay_carplay::audio::RingConsumer<i16>,format:catplay_carplay::audio::AudioStreamBasicDescription)->RtspResult<()>{
  if format.channels_per_frame<1||format.channels_per_frame>2||format.sample_rate<8000{return Err(RtspError::NotSupported);}
  self.rate=format.sample_rate;self.channels=format.channels_per_frame as usize;self.ring=Some(ring);
  log::info!("AUDIO_PCM input_rate={} channels={} output={}/{}",self.rate,self.channels,self.out_rate,if self.out_channels==1{"mono"}else{"stereo"});Ok(())
 }
 fn on_event(&mut self,event:catplay_carplay::audio::AudioPlayerEvent){
  use catplay_carplay::audio::{AudioPlayerEvent,RingSegment};
  match event {
   AudioPlayerEvent::Start | AudioPlayerEvent::Pause | AudioPlayerEvent::Stop | AudioPlayerEvent::Anchor=>{self.last_pcm=None;return;},
   AudioPlayerEvent::AddedDataToRing=>{},
  }
  let mut pcm=std::mem::take(&mut self.pcm_buf);
  pcm.clear();
  let out_frame_bytes=self.out_channels*2;
  if let Some(ring)=&self.ring {
   for segment in ring.iter_segments(16384,false){
    let samples=match segment {
     RingSegment::Data(samples)=>samples,
     RingSegment::Gap(count)|RingSegment::Overflow(count)=>{
      // Preserve brief holes in the RTP timeline without accumulating long outages.
      let frames=(count/self.channels).min(self.rate as usize/20);
      let phase=self.phase as usize+frames*self.out_rate as usize;
      pcm.resize(pcm.len()+(phase/self.rate as usize)*out_frame_bytes,0);
      self.phase=(phase%self.rate as usize) as u32;
      self.previous=[0;2];
      log::debug!("AUDIO_RTP_GAP input_samples={count} filled_frames={frames}");
      continue;
     }
     RingSegment::Underflow(_)=>continue,
    };
    if self.rate==self.out_rate&&self.channels==self.out_channels{
     pcm.reserve(samples.len()*2);
     for &sample in samples{pcm.extend_from_slice(&sample.to_le_bytes());}
     if let Some(tail)=samples.chunks_exact(self.channels).last(){self.previous=[tail[0],tail[self.channels-1]];}
    }else if self.rate==48000&&self.channels==2&&self.out_rate==16000&&self.out_channels==1{
     // Exact 3:1 stereo->mono boxcar decimation for AA Guidance (16 kHz mono).
     pcm.reserve((samples.len()/6)*2);
     for group in samples.chunks_exact(6){
      let sum:i32=group.iter().map(|&s|i32::from(s)).sum();
      let mono=(sum/6) as i16;
      pcm.extend_from_slice(&mono.to_le_bytes());
     }
     if let Some(tail)=samples.chunks_exact(2).last(){self.previous=[tail[0],tail[1]];}
    }else{
     for frame in samples.chunks_exact(self.channels){let current=[frame[0],frame[self.channels-1]];
      self.phase+=self.out_rate;while self.phase>=self.rate{self.phase-=self.rate;let f=1.0-self.phase as f64/self.out_rate as f64;
       if self.out_channels==1{
        let prev_m=(i32::from(self.previous[0])+i32::from(self.previous[1])) as f64*0.5;
        let curr_m=(i32::from(current[0])+i32::from(current[1])) as f64*0.5;
        let sample=(prev_m+(curr_m-prev_m)*f) as i16;
        pcm.extend_from_slice(&sample.to_le_bytes());
       }else{
        for ch in 0..2{let sample=(self.previous[ch] as f64+(current[ch] as f64-self.previous[ch] as f64)*f) as i16;pcm.extend_from_slice(&sample.to_le_bytes());}
       }
      }self.previous=current;
     }
    }
   }
  }
  if !pcm.is_empty(){
   let now=Instant::now();
   if let Some(previous)=self.last_pcm.replace(now){
    let gap=now.duration_since(previous);
    if gap>=Duration::from_millis(550){
     log::warn!("AUDIO_SOURCE_GAP gap_ms={} pcm_bytes={}",gap.as_millis(),pcm.len());
    }
   }
  }
  self.flush_pending();
  for chunk in pcm.chunks(7680){
   if self.pending.is_empty(){
    match self.socket.send(chunk){
     Ok(n) if n==chunk.len()=>continue,
     Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>{},
     result=>{log::warn!("AUDIO_PCM write failed: {result:?}");self.pcm_buf=pcm;return;},
    }
   }
   if self.pending.len()>=16 {
    self.pending.pop_front();
    self.dropped+=1;
    if self.dropped%64==1{log::warn!("AUDIO_QUEUE backlog bounded: dropped {} stale blocks",self.dropped);}
   }
   self.pending.push_back(chunk.to_vec());
  }
  self.flush_pending();
  self.pcm_buf=pcm;
 }
}
impl Audio {
 fn flush_pending(&mut self){
  while let Some(chunk)=self.pending.front(){
   match self.socket.send(chunk){
    Ok(n) if n==chunk.len()=>{self.pending.pop_front();},
    Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>break,
    result=>{log::warn!("AUDIO_PCM write failed: {result:?}");self.pending.clear();break;}
   }
  }
 }
}
struct Screen { pipe:UnixStream, start:Instant, config:Option<AvccConfigExtended>, last_slow:Option<Instant> }
#[async_trait]
impl ScreenReceiverSink for Screen {
 async fn init(&mut self)->RtspResult<()>{Ok(())}
 async fn set_avcc_config(&mut self,c:AvccConfigExtended)->RtspResult<()>{
  if c.hevc{return Err(RtspError::UnexpectedState("AA bridge currently requires H264".into()));}
  log::info!("CarPlay video configuration {}x{}",c.width,c.height);self.config=Some(c);Ok(())
 }
 async fn process_frame(&mut self,f:EncodedVideoFrame)->RtspResult<()>{
  if let Some(c)=f.config{self.set_avcc_config(c).await?;}
  let parameters=self.config.as_ref().map_or(&[][..],|c|c.avcc.sps_pps.as_slice());
  // ScreenReceiver normally supplies Annex-B. Reject unexpected formats explicitly.
  if !f.data.starts_with(&[0,0,0,1])&&!f.data.starts_with(&[0,0,1]){
   return Err(RtspError::UnexpectedState("Expected Annex-B CarPlay frame".into()));
  }
  let length=parameters.len()+f.data.len();
  if length>1024*1024{return Err(RtspError::UnexpectedState("Video frame exceeds 1 MiB".into()));}
  let stamp=f.pts.0.saturating_duration_since(self.start).as_micros() as u64;
  let mut header=[0u8;12];header[..4].copy_from_slice(&(length as u32).to_be_bytes());header[4..].copy_from_slice(&stamp.to_be_bytes());
  let mut slices=[IoSlice::new(&header),IoSlice::new(parameters),IoSlice::new(&f.data)];
  let mut pending=&mut slices[..];
  let started=Instant::now();
  tokio::time::timeout(Duration::from_secs(2),async {
   while !pending.is_empty(){
    let written=self.pipe.write_vectored(pending).await?;
    if written==0{return Err(Error::from(ErrorKind::WriteZero));}
    IoSlice::advance_slices(&mut pending,written);
   }
   Ok(())
  }).await.map_err(|_|RtspError::Timeout)?.map_err(|_|RtspError::Closed)?;
  let elapsed=started.elapsed();
  if elapsed>=Duration::from_millis(33)&&self.last_slow.is_none_or(|last|last.elapsed()>=Duration::from_secs(1)){
   self.last_slow=Some(Instant::now());
   log::warn!("VIDEO_RELAY_SLOW bytes={length} wait_ms={}",elapsed.as_millis());
  }
  log::debug!("CarPlay frame bytes={length} pts_us={stamp}");Ok(())
 }
}
impl AsyncShutdown for Screen {}
fn display_values()->Vec<u16> {
 let path=std::env::var("AA_VIDEO_SOCKET").unwrap_or_default()+".display";
 std::fs::read_to_string(path).unwrap_or_default().split_whitespace().filter_map(|v|v.parse::<u16>().ok()).collect()
}
fn display_size()->(u16,u16) {
 match display_values()[..] {[w,h,..] if w>=320&&h>=240=>(w,h),_=>(800,480)}
}
/// Width and height margins announced by the AA head unit for the selected video configuration.
fn display_margins()->(u16,u16) {
 match display_values()[..] {[_,_,mw,mh,..]=>(mw,mh),_=>(0,0)}
}
/// Android Auto VideoConfiguration height_margin is symmetric (half top, half bottom).
/// Auto mode splits margin_h equally between top and bottom; inset_top/inset_bottom override it.
fn visible_area_with_margin(options:&web::Options,w:u16,h:u16,margin_h:u16)->Option<catplay_carplay::msg::ViewArea> {
 let top=options.inset_top.unwrap_or(margin_h/2);let bottom=options.inset_bottom.unwrap_or(margin_h-margin_h/2);
 let edge=(options.sidebar==web::BarPosition::Bottom).then_some(1);
 let safe=if (top!=0||bottom!=0)&&u32::from(top)+u32::from(bottom)+120<=u32::from(h){
  Some(catplay_carplay::msg::SafeArea{origin_x_pixels:0,origin_y_pixels:top.into(),width_pixels:w.into(),height_pixels:(h-top-bottom).into()})
 }else{None};
 if safe.is_none()&&edge.is_none(){return None;}
 Some(catplay_carplay::msg::ViewArea{origin_x_pixels:0,origin_y_pixels:0,width_pixels:w.into(),height_pixels:h.into(),safe_area:safe,view_area_status_bar_edge:edge})
}
fn visible_area(options:&web::Options,w:u16,h:u16)->Option<catplay_carplay::msg::ViewArea> {
 let (_,margin_h)=display_margins();
 visible_area_with_margin(options,w,h,margin_h)
}
#[cfg(test)]
#[test]
fn bottom_bar_has_a_view_area_on_a_full_screen() {
 let mut options=web::Options{sidebar:web::BarPosition::Bottom,inset_top:Some(0),inset_bottom:Some(0),..Default::default()};
 let area=visible_area_with_margin(&options,800,480,0).unwrap();
 assert_eq!(area.view_area_status_bar_edge,Some(1));
 assert_eq!((area.width_pixels,area.height_pixels),(800,480));
 assert!(area.safe_area.is_none());
 options.inset_bottom=Some(40);
 assert_eq!(visible_area_with_margin(&options,800,480,0).unwrap().safe_area.unwrap().height_pixels,440);
 options.inset_bottom=Some(0);
 options.sidebar=web::BarPosition::Right;
 assert!(visible_area_with_margin(&options,800,480,0).is_none());
 /* Auto mode splits AA height_margin symmetrically (top = margin/2, bottom = margin - margin/2). */
 let auto_opts=web::Options::default();
 let auto_area=visible_area_with_margin(&auto_opts,800,480,32).unwrap();
 let safe=auto_area.safe_area.unwrap();
 assert_eq!((safe.origin_x_pixels,safe.origin_y_pixels,safe.width_pixels,safe.height_pixels),(0,16,800,448));
 assert!(visible_area_with_margin(&auto_opts,800,480,0).is_none());
}

const DISPLAY_UUID: &str = "92EF87D8-88E6-48CD-83A0-7498C886774C";
fn touch_device()->catplay_carplay::msg::HidDevice {
 catplay_carplay::msg::HidDevice {display_uuid:DISPLAY_UUID.into(),name:"Bridge touchscreen".into(),uuid:"B9B32748-EDAB-494A-A3F1-65A16450C1DE".into(),hid_descriptor:catplay_hid::TouchScreenSingleReport::descriptor(display_size().0-1,display_size().1-1).to_vec().into(),..Default::default()}
}
fn media_device()->catplay_carplay::msg::HidDevice {
 // Keep the original report indices, then extend the consumer array with
 // volume up/down, mute, stop, rewind and fast forward.
 let base=catplay_hid::MediaButtonsReport::descriptor();
 let mut descriptor=base[..33].to_vec();
 descriptor[9]=12;
 for usage in [0xe9,0xea,0xe2,0xb7,0xb4,0xb3] {
  descriptor.extend_from_slice(&[0x0a,usage,0]);
 }
 descriptor.extend_from_slice(&base[33..]);
 catplay_carplay::msg::HidDevice {
  name:"Bridge media buttons".into(),
  display_uuid:DISPLAY_UUID.into(),
  uuid:"33846C13-C691-4746-A77B-A44A77686485".into(),
  hid_descriptor:descriptor.into(),
  ..Default::default()
 }
}
fn telephony_device() -> catplay_carplay::msg::HidDevice {
 catplay_carplay::msg::HidDevice {
  name:"Bridge call controls".into(),
  display_uuid:DISPLAY_UUID.into(),
  uuid:"E36D240B-BF27-4619-8097-77456EDFB851".into(),
  hid_descriptor:catplay_hid::TelephonyButtonsReport::descriptor().into(),
  ..Default::default()
 }
}
fn knob_device() -> catplay_carplay::msg::HidDevice {
 catplay_carplay::msg::HidDevice {
  name:"Bridge rotary controls".into(),
  display_uuid:DISPLAY_UUID.into(),
  uuid:"A038A344-6236-457E-B6BD-0BA703E937AD".into(),
  hid_descriptor:catplay_hid::KnobReport::descriptor().into(),
  ..Default::default()
 }
}
static TOUCH_GENERATION:std::sync::atomic::AtomicU32=std::sync::atomic::AtomicU32::new(0);
static ACTIVE_CARPLAY_SESSIONS:std::sync::atomic::AtomicU32=std::sync::atomic::AtomicU32::new(0);
fn car_night_mode(socket:&str)->Option<bool>{
 match std::fs::read_to_string(format!("{socket}.night")).ok()?.trim() {
  "1"=>Some(true),
  "0"=>Some(false),
  _=>None,
 }
}
fn parse_bluetooth_mac(raw:&str)->Option<String>{
 let mac=raw.trim();
 if mac.len()!=17{return None;}
 for (i,b) in mac.bytes().enumerate(){
  if i%3==2{if b!=b':'{return None;}}
  else if !b.is_ascii_hexdigit(){return None;}
 }
 Some(mac.to_ascii_uppercase())
}
fn car_bluetooth_mac(socket:&str)->Option<String>{
 parse_bluetooth_mac(&std::fs::read_to_string(format!("{socket}.btmac")).ok()?)
}
fn guidance_format(socket:&str)->(u32,usize){
 let text=std::fs::read_to_string(format!("{socket}.guidance.fmt")).unwrap_or_default();
 let mut nums=text.split_whitespace().filter_map(|s|s.parse::<u32>().ok());
 match (nums.next(),nums.next()) {
  (Some(r),Some(c)) if (r==16000||r==48000)&&(c==1||c==2)=>(r,c as usize),
  _=>(16000,1),
 }
}
fn effective_night_mode(theme:&str,car_night:Option<bool>)->Option<bool>{
 match theme {
  "night"=>Some(true),
  "day"=>Some(false),
  _=>car_night,
 }
}
struct Receiver { socket:String, input_task:Option<tokio::task::JoinHandle<()>>, touch_generation:Option<u32>, handle:Option<catplay_carplay::carplay_rx::AirPlayReceiverHandleRef>, session_active:bool, mfi:Option<catplay_mfi::MfiDeficeRef>, gps:gps_iap2::GpsHub }
fn configure_oem_icon(info:&mut InfoMessageResponse,options:&web::Options){
 use std::hash::{Hash,Hasher};
 let name=web::oem_name();
 let (logo,source)=web::logo();
 let icons=web::projection_icons(&logo);
 let mut revision=std::collections::hash_map::DefaultHasher::new();
 name.hash(&mut revision);icons.hash(&mut revision);format!("{options:?}").hash(&mut revision);
 info.firmware_revision=format!("CarFluxoid-{:016x}",revision.finish());
 let digest=openssl::sha::sha256(&logo);
 let fingerprint=digest[..8].iter().map(|byte|format!("{byte:02x}")).collect::<String>();
 eprintln!("CP_OEM_ARTWORK source={source} label={name:?} sha256_prefix={fingerprint} visible={} sizes={:?}",
  options.return_button,icons.iter().map(|(size,image)|(*size,image.len())).collect::<Vec<_>>());
 info.oem_icon=None;
 info.oem_icon_label=Some(name);
 info.oem_icon_visible=options.return_button.into();
 info.oem_icons=icons.into_iter().map(|(size,image)|catplay_carplay::msg::OemIcon{
  image_data:image.into(),width_pixels:size,height_pixels:size,prerendered:true.into()
 }).collect();
}
impl Drop for Receiver {fn drop(&mut self){
 if self.session_active{
  self.session_active=false;
  ACTIVE_CARPLAY_SESSIONS.fetch_sub(1,std::sync::atomic::Ordering::SeqCst);
 }
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
  if let Some(parent)=std::path::Path::new(&path).parent() { let _=std::fs::create_dir_all(parent); }
  let _=std::fs::remove_file(&path);
  let input=tokio::net::UnixDatagram::bind(&path).map_err(|err|{
   log::error!("TOUCH_SOCKET_BIND_FAILED path={path}: {err}");
   RtspError::Closed
  })?;
  self.touch_generation=Some(generation);
  let gps=self.gps.clone();
  self.input_task=Some(tokio::spawn(async move {
   let _guard=path;

   use catplay_carplay::msg::{Command, CommandRequestSiri, CommandSetNightMode, SiriAction};
   let mut packet=[0u8;64];
   let mut siri_pressed=false;
   let mut knob_buttons=0;
   let mut media_release:Option<tokio::task::JoinHandle<()>>=None;
   let mut tel_release:Option<tokio::task::JoinHandle<()>>=None;
   let mut siri_release:Option<tokio::task::JoinHandle<()>>=None;
   let (w,h)=display_size();
   let touch_hid=touch_device();
   let media_hid=media_device();
   let telephony_hid=telephony_device();
   let knob_hid=knob_device();
   while let Ok(n)=input.recv(&mut packet).await {
    // The AA sender asks for a fresh IDR after the head unit dropped or refused frames.
    if n==1&&packet[0]==0x10{handle.request_keyframe();log::info!("KEYFRAME requested by AA sender");continue;}
    if n==29&&packet[0]==0x27{
     if let Some(loc)=gps_iap2::CarLocation::from_packet(&packet[..n]){gps.on_location(loc);}
     continue;
    }
    if n==6&&packet[0]==0x28{
     if let Some(speed)=gps_iap2::parse_speed_packet(&packet[..n]){gps.on_speed(speed);}
     continue;
    }
    if n==2 {
     let state=packet[1];
     if packet[0]==0x26 {
      if web::options().theme=="auto" {
       let night=state!=0;
       handle.send_command_and_forget(Command::SetNightMode(CommandSetNightMode{night_mode:night}));
       eprintln!("CP_NIGHT_MODE night={night}");
      }
      continue;
     }
     if state!=0 {eprintln!("CP_INPUT type=0x{:02x} state={state}",packet[0]);}
     let now=Instant::now();
     match packet[0] {
      0x20 if state<=12 => {
       if let Some(t)=media_release.take(){t.abort();}
       handle.send_hid_report(now,&media_hid,&[state]);
       if state!=0 {
        // Uconnect VP2 sends isDown=1 without release for AA media keys; auto-release after 50 ms.
        let h2=handle.clone();let m2=media_hid.clone();
        media_release=Some(tokio::spawn(async move{
         tokio::time::sleep(Duration::from_millis(50)).await;
         h2.send_hid_report(Instant::now(),&m2,&[0]);
        }));
       }
      }
      0x21 if state<=4 => {
       if let Some(t)=tel_release.take(){t.abort();}
       let report=catplay_hid::TelephonyButtonsReport::new(state.into()).to_bytes();
       handle.send_hid_report(now,&telephony_hid,&report);
       if state!=0 {
        // Auto-release after 50 ms for head units (Uconnect VP2) that send isDown=1 without release.
        let h2=handle.clone();let t2=telephony_hid.clone();
        tel_release=Some(tokio::spawn(async move{
         tokio::time::sleep(Duration::from_millis(50)).await;
         let up=catplay_hid::TelephonyButtonsReport::new(catplay_hid::TelephonyButton::Up).to_bytes();
         h2.send_hid_report(Instant::now(),&t2,&up);
        }));
       }
      }
      0x22 if state==2 => {
       // Uconnect VP2 VR long-press arrives without release: Prewarm -> ButtonDown -> ButtonUp.
       if let Some(t)=siri_release.take(){t.abort();}
       siri_pressed=false;
       handle.send_command_and_forget(Command::RequestSiri(CommandRequestSiri{siri_action:SiriAction::Prewarm}));
       handle.send_command_and_forget(Command::RequestSiri(CommandRequestSiri{siri_action:SiriAction::ButtonDown}));
       let h2=handle.clone();
       siri_release=Some(tokio::spawn(async move{
        tokio::time::sleep(Duration::from_millis(50)).await;
        h2.send_command_and_forget(Command::RequestSiri(CommandRequestSiri{siri_action:SiriAction::ButtonUp}));
       }));
      }
      0x22 if state<=1 && siri_pressed!=(state==1) => {
       if let Some(t)=siri_release.take(){t.abort();}
       siri_pressed=state==1;
       if siri_pressed {
        handle.send_command_and_forget(Command::RequestSiri(CommandRequestSiri{siri_action:SiriAction::Prewarm}));
       }
       let siri_action=if siri_pressed {SiriAction::ButtonDown} else {SiriAction::ButtonUp};
       handle.send_command_and_forget(Command::RequestSiri(CommandRequestSiri{siri_action}));
      }
      0x23 if matches!(state,0|1|2|4) => {
       knob_buttons=state;
       handle.send_hid_report(now,&knob_hid,&[knob_buttons,0,0,0]);
      }
      0x24 if state<=4 => {
       let (x,y)=match state {1=>(0,-127i8),2=>(0,127),3=>(-127,0),4=>(127,0),_=>(0,0)};
       handle.send_hid_report(now,&knob_hid,&[knob_buttons,x as u8,y as u8,0]);
      }
      0x25 => handle.send_hid_report(now,&knob_hid,&[knob_buttons,0,0,state]),
      _=>{}
     }
     continue;
    }
    if n!=5||packet[0]>1{continue;}
    let x=u16::from_le_bytes([packet[1],packet[2]]);let y=u16::from_le_bytes([packet[3],packet[4]]);
    if x>=w||y>=h{continue;}
    handle.send_hid_report(Instant::now(),&touch_hid,&packet[..5]);
    log::debug!("TOUCH sent pressed={} x={x} y={y}",packet[0]);
   }
  }));Ok(())
 }

 async fn on_record(&mut self)->RtspResult<()>{
  if !self.session_active{
   self.session_active=true;
   ACTIVE_CARPLAY_SESSIONS.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
  }
  let theme=web::options().theme;
  if let Some(night)=effective_night_mode(&theme,car_night_mode(&self.socket)) {
   if let Some(handle)=&self.handle {
    handle.send_command_and_forget(catplay_carplay::msg::Command::SetNightMode(catplay_carplay::msg::CommandSetNightMode{night_mode:night}));
    eprintln!("CP_NIGHT_MODE initial night={night} theme={theme}");
   }
  }
  Ok(())
 }
 async fn negotiate_features(&mut self,offered:&[catplay_carplay::msg::ControllerFeature])->Vec<catplay_carplay::msg::ControllerFeature>{
  // catplay handles iAP2 over RTSP iAPSendMessage, not the separate iAPChannel stream;
  // enabling IApChannel causes iOS to send an unsupported SETUP stream (406).
  eprintln!("CP_FEATURES offered={offered:?} enabled=[]");
  vec![]
 }
 async fn open_iap2(&mut self)->Option<catplay_iap2_client::CsmSessionBox>{
  Some(Box::new(gps_iap2::GpsIap2Session::new(self.mfi.clone(),web::bluetooth_name(),self.gps.clone(),self.socket.clone())))
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
  info.right_hand_drive=(options.sidebar==web::BarPosition::Right).into();
  info.night_mode=effective_night_mode(&options.theme,car_night_mode(&self.socket)).map(Into::into);
  if let Some(mac)=car_bluetooth_mac(&self.socket) {
   if !info.bluetooth_ids.iter().any(|existing|existing.eq_ignore_ascii_case(&mac)) {
    info.bluetooth_ids.push(mac);
   }
   eprintln!("CP_BT_IDS ids={:?}",info.bluetooth_ids);
  }
  configure_oem_icon(info,&options);
  // Prefer native 48 kHz stereo to avoid 44.1->48 kHz resampling on the Pi CPU.
  for fmt in &mut info.audio_formats {
   if let Some(out)=&mut fmt.audio_output_formats {
    if out.contains(catplay_carplay::msg::AudioFormat::PCM_48000_STEREO) {
     out.remove(catplay_carplay::msg::AudioFormat::PCM_44100_STEREO);
    }
    if out.contains(catplay_carplay::msg::AudioFormat::AAC_LC_48000_STEREO) {
     out.remove(catplay_carplay::msg::AudioFormat::AAC_LC_44100_STEREO);
    }
   }
  }
  info.hid_devices.push(touch_device());
  info.hid_devices.push(media_device());
  info.hid_devices.push(telephony_device());
  info.hid_devices.push(knob_device());
  let (w,h)=display_size();let (physical_w,physical_h)=options.physical_size(w,h);eprintln!("CP_DISPLAY negotiated {}x{} scale={} physical={}x{}mm",w,h,options.scale_percent,physical_w,physical_h);
  let view_area=visible_area(&options,w,h);eprintln!("CP_VIEW_AREA margins={:?} safe={:?}",display_margins(),view_area.as_ref().and_then(|v|v.safe_area.as_ref()).map(|a|(a.origin_y_pixels,a.height_pixels)));
  use catplay_carplay::msg::DisplayFeature;
  let touch_features=if options.assist_buttons{DisplayFeature::LOWFI_TOUCH}else{DisplayFeature::HIGHFI_TOUCH};
  info.displays=vec![Display{width_pixels:w.into(),height_pixels:h.into(),width_physical:physical_w.into(),height_physical:physical_h.into(),max_fps:Some(options.fps.into()),primary_input_device:Some(catplay_carplay::msg::PrimaryInputDevice::TouchScreen),features:touch_features|DisplayFeature::KNOBS,uuid:"92EF87D8-88E6-48CD-83A0-7498C886774C".into(),initial_view_area:view_area.as_ref().map(|_|0),view_areas:view_area.map(|v|vec![v]),..Default::default()}];
 }
 async fn open_microphone(&mut self,_:catplay_carplay::msg::StreamType,_:catplay_carplay::msg::AudioType,_:catplay_carplay::audio::AudioStreamBasicDescription)->RtspResult<catplay_carplay::audio::AudioRecorderBox<i16>>{
  Ok(Box::new(microphone::Microphone::new(&self.socket)?))
 }
 async fn open_audio(&mut self,_:Duration,stream_type:catplay_carplay::msg::StreamType,_:catplay_carplay::msg::AudioType,_:catplay_carplay::msg::AudioFormat,_:catplay_carplay::audio::AudioStreamBasicDescription,_:bool)->RtspResult<catplay_carplay::audio::AudioPlayerBox<i16>>{
  let socket=std::os::unix::net::UnixDatagram::unbound().map_err(|_|RtspError::Closed)?;
  let (out_rate,out_channels)=if stream_type==catplay_carplay::msg::StreamType::AltAudio
   && socket.connect(format!("{}.guidance",self.socket)).is_ok()
  {
   let (r,c)=guidance_format(&self.socket);
   eprintln!("AUDIO_GUIDANCE_ROUTED out_rate={r} out_channels={c}");
   (r,c)
  }else{
   socket.connect(format!("{}.audio",self.socket)).map_err(|_|RtspError::Closed)?;
   (48000,2)
  };
  socket.set_nonblocking(true).map_err(|_|RtspError::Closed)?;
  Ok(Box::new(Audio{socket,ring:None,rate:48000,channels:2,out_rate,out_channels,phase:0,previous:[0,0],pending:std::collections::VecDeque::new(),pcm_buf:Vec::with_capacity(16384),dropped:0,last_pcm:None}))
 }
 async fn open_screen(&mut self,_:Duration)->RtspResult<ScreenReceiverSinkBox>{
  let pipe=UnixStream::connect(&self.socket).await.map_err(|_|RtspError::Closed)?;
  Ok(Box::new(Screen{pipe,start:Instant::now(),config:None,last_slow:None}))
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
 if std::env::args().nth(1).as_deref()==Some("--dashboard"){
  let mut paths=carfluxoid_dashboard_ui::state::Paths::default();
  let args:Vec<String>=std::env::args().skip(2).collect();
  let mut i=0;
  while i<args.len(){
   match args[i].as_str(){
    "--settings" if i+1<args.len()=>{paths.settings=args[i+1].clone().into();i+=2;}
    "--assets" if i+1<args.len()=>{paths.assets=args[i+1].clone().into();i+=2;}
    "--runtime" if i+1<args.len()=>{paths.runtime=args[i+1].clone().into();i+=2;}
    "--bridge" if i+1<args.len()=>{paths.bridge=args[i+1].clone().into();i+=2;}
    _=>{i+=1;}
   }
  }
  carfluxoid_dashboard_ui::run_daemon(paths).map_err(|e|std::io::Error::other(e))?;
  return Ok(());
 }
 if std::env::args().nth(1).as_deref()==Some("--oem-info"){
  use catplay_carplay::rtsp_frame::{HttpStatus,RtspResponse};
  use std::io::Write;
  let mut info=InfoMessageResponse::default();
  configure_oem_icon(&mut info,&web::options());
  let mut response=RtspResponse::new(None,HttpStatus::Ok);
  response.set_plist(info)?;
  std::io::stdout().lock().write_all(&response.payload)?;
  return Ok(());
 }
 // Preserve the single-core dongle default; the Pi profile opts into workers.
 let workers=std::env::var("AA_RUNTIME_WORKERS").ok().and_then(|s|s.parse::<usize>().ok()).unwrap_or(1).clamp(1,4);
 let mut builder=if workers==1 {tokio::runtime::Builder::new_current_thread()} else {
  let mut builder=tokio::runtime::Builder::new_multi_thread();builder.worker_threads(workers);builder
 };
 let runtime=builder.enable_all().build()?;
 eprintln!("CP_BOOT 2: runtime ready");
 runtime.block_on(run())
}
async fn run()->Result<(),Box<dyn std::error::Error>>{
 eprintln!("CP_BOOT 3: initializing logger");
 env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(if std::env::var("AA_DIAGNOSTIC").as_deref()==Ok("1"){"info"}else{"warn"})).init();
 eprintln!("CP_BOOT 4: logger ready");
 let args:Vec<_>=std::env::args().collect();
 if args.get(1).map(String::as_str)==Some("--self-test"){
  carfluxoid_dashboard_ui::self_test().map_err(|e|std::io::Error::other(e))?;
  eprintln!("CP_SELF_TEST_OK");
  return Ok(());
 }
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
 let receiver_name=web::bluetooth_name();
 let session_mfi:catplay_mfi::MfiDeficeRef=mfi.clone();
 let mut receiver=CarPlayWirelessGadget::new(homekit,Some(mfi),AirPlayServerShared::new(),&receiver_name,"hci0","wlan0",ssid,password.as_deref(),password.is_some(),None,Some(last_peer.to_str().ok_or("peer cache path")?),move||Receiver{socket:socket.clone(),input_task:None,touch_generation:None,handle:None,session_active:false,mfi:Some(session_mfi.clone()),gps:gps_iap2::GpsHub::new()});
 log::info!("Standalone CarPlay receiver starting; no car-side CarPlay dependency");
 let mut bt_quiet=false;
 loop {
  let want_quiet=ACTIVE_CARPLAY_SESSIONS.load(std::sync::atomic::Ordering::SeqCst)>0;
  if want_quiet!=bt_quiet {
   let discoverable=!want_quiet;
   match pairing.set_discoverable("hci0",discoverable).await {
    Ok(())=>{
     let _=pairing.set_pairable("hci0",discoverable).await;
     bt_quiet=want_quiet;
     eprintln!("BT_QUIET enabled={bt_quiet}");
    }
    Err(e)=>log::warn!("BT_QUIET update failed (want_quiet={want_quiet}): {e}"),
   }
  }
  if let Some(request)=pairing.get_pairing_request(){
   log::info!("Bluetooth pairing request: {request:?}; accepting for bridge test");
   request.accept();
  }
  let _=receiver.reconcile().await;
  tokio::select!{_=tokio::signal::ctrl_c()=>break,_=receiver.sleep()=>{},_=tokio::time::sleep(Duration::from_millis(100))=>{}}
 }
 if bt_quiet {
  let _=pairing.set_pairable("hci0",true).await;
  let _=pairing.set_discoverable("hci0",true).await;
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
  let old=Receiver{socket:base.clone(),input_task:None,touch_generation:Some(41),handle:None,session_active:false,mfi:None,gps:gps_iap2::GpsHub::new()};
  let new=Receiver{socket:base.clone(),input_task:None,touch_generation:Some(42),handle:None,session_active:false,mfi:None,gps:gps_iap2::GpsHub::new()};
  drop(old);assert!(std::os::unix::net::UnixDatagram::bind(&path).is_err());
  drop(new);let mut event=[0u8;1];control.recv(&mut event).unwrap();assert_eq!(event,[4]);
  let _replacement=std::os::unix::net::UnixDatagram::bind(&path).unwrap();let _=std::fs::remove_file(path);let _=std::fs::remove_file(control_path);
 }
 #[test]fn effective_night_mode_respects_manual_and_auto(){
  assert_eq!(effective_night_mode("auto",Some(true)),Some(true));
  assert_eq!(effective_night_mode("auto",Some(false)),Some(false));
  assert_eq!(effective_night_mode("auto",None),None);
  assert_eq!(effective_night_mode("night",Some(false)),Some(true));
  assert_eq!(effective_night_mode("day",Some(true)),Some(false));
 }
}

#[cfg(test)]
mod audio_gap_tests {
 use super::*;
 use catplay_carplay::audio::{AudioPlayer,AudioPlayerEvent,RingBuffer};

 #[test]
 fn forwards_silence_for_missing_input_frames() {
  let (socket,peer)=std::os::unix::net::UnixDatagram::pair().unwrap();
  peer.set_nonblocking(true).unwrap();
  let (mut input,ring)=RingBuffer::<i16>::spsc(8192);
  let mut audio=Audio{socket,ring:Some(ring),rate:48000,channels:2,out_rate:48000,out_channels:2,phase:0,
   previous:[0,0],pending:Default::default(),pcm_buf:Vec::new(),dropped:0,last_pcm:None};
  input.advance_timeline(960);
  input.write(&[123,456]);
  audio.on_event(AudioPlayerEvent::AddedDataToRing);
  let mut bytes=[0u8;4096];
  let count=peer.recv(&mut bytes).unwrap();
  assert_eq!(count,1924);
  assert!(bytes[..1920].iter().all(|&byte|byte==0));
  assert_eq!(&bytes[1920..count],&[123,0,200,1]);
 }

 #[test]
 fn decimates_48k_stereo_to_16k_mono_for_guidance() {
  let (socket,peer)=std::os::unix::net::UnixDatagram::pair().unwrap();
  peer.set_nonblocking(true).unwrap();
  let (mut input,ring)=RingBuffer::<i16>::spsc(8192);
  let mut audio=Audio{socket,ring:Some(ring),rate:48000,channels:2,out_rate:16000,out_channels:1,phase:0,
   previous:[0,0],pending:Default::default(),pcm_buf:Vec::new(),dropped:0,last_pcm:None};
  input.write(&[600,600,600,600,600,600,-300,-300,-300,-300,-300,-300]);
  audio.on_event(AudioPlayerEvent::AddedDataToRing);
  let mut bytes=[0u8;64];
  let count=peer.recv(&mut bytes).unwrap();
  assert_eq!(count,4);
  assert_eq!(i16::from_le_bytes([bytes[0],bytes[1]]),600);
  assert_eq!(i16::from_le_bytes([bytes[2],bytes[3]]),-300);
 }

 #[test]
 fn validates_bluetooth_mac_addresses() {
  assert_eq!(parse_bluetooth_mac("02:1a:2b:3c:4d:5e\n").as_deref(),Some("02:1A:2B:3C:4D:5E"));
  assert!(parse_bluetooth_mac("02:1A:2B:3C:4D").is_none());
  assert!(parse_bluetooth_mac("ZZ:1A:2B:3C:4D:5E").is_none());
 }
}
