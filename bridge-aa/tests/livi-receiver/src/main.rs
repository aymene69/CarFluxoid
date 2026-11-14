#[path = "../../../../work/livi/native/crates/livi-aa-stack/src/protocol.rs"] mod protocol;
#[path = "../../../../work/livi/native/crates/livi-aa-stack/src/codec.rs"] mod codec;
#[path = "../../../../work/livi/native/crates/livi-aa-stack/src/control.rs"] mod control;
#[path = "../../../../work/livi/native/crates/livi-aa-stack/src/wire.rs"] mod wire;
#[path = "../../../../work/livi/native/crates/livi-aa-stack/src/log.rs"] mod log;
pub use livi_aa_stack::{channels,config,discovery,sensors};
use std::{sync::Arc,io::Write};
use tokio::{net::{TcpListener,UnixListener,UnixStream},io::{AsyncReadExt,AsyncWriteExt},sync::{watch,Notify}};
use livi_session_io::link::{self,Item};
use protocol::{Session,Out};
async fn apply(node:&mut UnixStream, outs:Vec<Out>){
 for o in outs { let bytes=match o {Out::Send(f)=>Some(link::encode_message(f.ch,f.flags,f.msg_id,&f.payload)),Out::Control(v)=>Some(link::encode_control(&v.to_string())),Out::Event(e)=>{println!("EVENT {e:?}");None},_=>None};if let Some(b)=bytes{node.write_all(&b).await.unwrap();}}
}
trait TestIo: tokio::io::AsyncRead+tokio::io::AsyncWrite+Unpin+Send {}
impl<T:tokio::io::AsyncRead+tokio::io::AsyncWrite+Unpin+Send> TestIo for T {}
#[tokio::main]
async fn main(){
 let root=std::env::args().nth(1).unwrap_or("/tmp/aa-lab".into());std::fs::create_dir_all(&root).unwrap();
 let np=format!("{root}/node.sock");let fp=format!("{root}/feed.sock");let _=std::fs::remove_file(&np);let _=std::fs::remove_file(&fp);
 let nl=UnixListener::bind(&np).unwrap();let fl=UnixListener::bind(&fp).unwrap();let cancel=Arc::new(Notify::new());let end=cancel.clone();
 let touch_socket=tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
 let touch_port=touch_socket.local_addr().unwrap().port();
 let live=std::env::var_os("AA_LIVE_VIEW").is_some();
 let audio_output=format!("{root}/received.pcm");
 let output=format!("{root}/received.h264");
 tokio::spawn(async move {
  let (mut s,_)=fl.accept().await.unwrap();
  let mut fr=livi_output_proto::feed::Framer::new();let mut b=[0;65536];
  let mut f=std::fs::File::create(output).unwrap();let mut count=0;
  let mut player=if live {Some(tokio::process::Command::new(std::env::var("AA_VIEWER").unwrap_or("/opt/homebrew/bin/ffplay".into()))
   .env("AA_TOUCH_PORT",touch_port.to_string())
   .args(["-hide_banner","-loglevel","warning","-window_title","Android Auto — sortie du dongle","-autoexit","-flags","low_delay","-probesize","32","-analyzeduration","0","-f","h264","-framerate","30","-i","pipe:0"])
   .stdin(std::process::Stdio::piped()).kill_on_drop(true).spawn().expect("ffplay"))}else{None};
  let mut audio_file=std::fs::File::create(audio_output).unwrap();let mut audio_blocks=0u64;
  let mut audio_player=if live{Some(tokio::process::Command::new("/opt/homebrew/bin/ffplay").args(["-nodisp","-autoexit","-loglevel","warning","-f","s16le","-ar","48000","-ch_layout","stereo","-i","pipe:0"]).stdin(std::process::Stdio::piped()).kill_on_drop(true).spawn().unwrap())}else{None};
  while let Ok(n)=s.read(&mut b).await {
   if n==0{break}fr.push(&b[..n]);
   while let Some(Ok(rec))=fr.next_record(){
    if rec.kind==2{audio_file.write_all(&rec.payload).unwrap();audio_file.flush().unwrap();audio_blocks+=1;let peak=rec.payload.chunks_exact(2).map(|b|i16::from_le_bytes([b[0],b[1]]).unsigned_abs()).max().unwrap_or(0);if audio_blocks%100==1{println!("AUDIO_LEVEL peak={peak} block={audio_blocks}");}if let Some(child)=audio_player.as_mut(){if let Err(e)=child.stdin.as_mut().unwrap().write_all(&rec.payload).await{eprintln!("AUDIO_PLAYER_ERROR {e}");}}println!("RECEIVED_AUDIO bytes={}",rec.payload.len());}
    if rec.kind==1 {
    f.write_all(&rec.payload).unwrap();f.flush().unwrap();count+=1;
    if let Some(child)=player.as_mut(){if child.stdin.as_mut().unwrap().write_all(&rec.payload).await.is_err(){end.notify_one();return;}}
    println!("RECEIVED_VIDEO {count} bytes={}",rec.payload.len());
    if !live&&count>=10{end.notify_one();return;}
   }}
  }
  end.notify_one();
 });
 let (io,addr)=if std::env::var_os("AA_TEST_USB").is_some(){
 println!("WAITING USB CatPlay AA Lab (fermer LIVI et DHU)");
 let info=loop {let found=nusb::list_devices().await.unwrap().find(|d|d.vendor_id()==0x18d1&&d.product_id()==0x2d00&&d.serial_number().unwrap_or("").starts_with("CATPLAY_AA_LAB"));if let Some(d)=found{break d;}tokio::time::sleep(std::time::Duration::from_millis(200)).await;};
 let dev=info.open().await.unwrap();let pipe=livi_session_io::usb::open_pipe(&dev).await.unwrap();
 let usb=livi_session_io::usb::UsbStream::new(pipe);
 let (local,relay)=tokio::io::duplex(131072);let (mut ur,mut uw)=tokio::io::split(usb);let (mut lr,mut lw)=tokio::io::split(relay);
 let mut sent=std::fs::File::create(format!("{root}/host-tx.bin")).unwrap();
 let mut received=std::fs::File::create(format!("{root}/host-rx.bin")).unwrap();
 tokio::spawn(async move {let mut b=[0;65536];loop {let n=match lr.read(&mut b).await{Ok(n) if n>0=>n,_=>break};sent.write_all(&b[..n]).unwrap();sent.flush().unwrap();if uw.write_all(&b[..n]).await.is_err(){break}tokio::time::sleep(std::time::Duration::from_millis(8)).await;}});
 tokio::spawn(async move {let mut b=[0;65536];loop {let n=match ur.read(&mut b).await{Ok(n) if n>0=>n,_=>break};received.write_all(&b[..n]).unwrap();received.flush().unwrap();if lw.write_all(&b[..n]).await.is_err(){break}}});
 (Box::new(local) as Box<dyn TestIo>,"127.0.0.1:5278".parse::<std::net::SocketAddr>().unwrap())
 }else{let tcp=TcpListener::bind("0.0.0.0:5278").await.unwrap();println!("READY AA port 5278");let (io,addr)=tcp.accept().await.unwrap();(Box::new(io) as Box<dyn TestIo>,addr)};let p=np.clone();let c=cancel.clone();tokio::spawn(async move {livi_aa::session::run(io,livi_aa::session::Peer{label:addr.to_string(),ip:addr.ip()},nl,p,c).await;});
 let mut node=UnixStream::connect(np).await.unwrap();let (_tx,rx)=watch::channel(config::AaConfig{display_width:800,display_height:480,video_width:Some(800),video_height:Some(480),hevc_supported:std::env::var_os("AA_TEST_HEVC").is_some(),cluster_enabled:true,cluster_width:1280,cluster_height:720,disable_audio_output:false,..Default::default()});
 let (mut state,first)=Session::new(rx,"AA Lab");apply(&mut node,first).await;
 node.write_all(&link::encode_control(&serde_json::json!({"type":"sink","feed":fp,"video":[{"ch":3,"id":1,"codec":"h264"}],"audio":[{"ch":4,"id":2}]}).to_string())).await.unwrap();
 let mut fr=link::Framer::default();let mut b=[0;65536];let mut tick=tokio::time::interval(std::time::Duration::from_millis(1500));let deadline=tokio::time::sleep(std::time::Duration::from_secs(if std::env::var_os("AA_BRIDGE").is_some(){620}else if live {130}else{25}));tokio::pin!(deadline);
 let mut touch_buffer=[0u8;64];
 loop {tokio::select! {event=touch_socket.recv(&mut touch_buffer)=>{
  if let Ok(n)=event{if let Ok(text)=std::str::from_utf8(&touch_buffer[..n]){
   let values:Vec<u32>=text.split_whitespace().filter_map(|s|s.parse().ok()).collect();
   if values.len()==3&&values[0]<=2&&values[1]<800&&values[2]<480{
    if let Some(f)=channels::input::touch(0,values[0],&[channels::input::TouchPointer{x:values[1] as i64,y:values[2] as i64,id:0}],0){apply(&mut node,vec![Out::Send(f)]).await;}
   }
  }}
 },n=node.read(&mut b)=>{let n=n.unwrap();if n==0{break}fr.push(&b[..n]);while let Some(Ok(item))=fr.next_item(){let out=match item{Item::Message{ch,msg_id,payload,..}=>state.on_message(ch,msg_id,&payload),Item::Control(j)=>{println!("CONTROL {j}");state.on_link_control(&serde_json::from_str(&j).unwrap())}};apply(&mut node,out).await;}},_=tick.tick()=>apply(&mut node,state.on_ping_tick()).await,_=cancel.notified()=>break,_=&mut deadline=>{println!("TIMEOUT");std::process::exit(1)}}}
 println!("RECEIVER FINISHED");
}
