use std::{io::{Read,Write},net::{TcpListener,TcpStream},path::{Path,PathBuf},process::{Command,Stdio},time::{Duration,SystemTime,UNIX_EPOCH}};
const HTML:&str=include_str!("../assets/settings.html");
const APP_VERSION:&str=match option_env!("CARFLUXOID_VERSION"){Some(v)=>v,None=>"dev"};
const APP_BUILD:&str=match option_env!("CARFLUXOID_BUILD"){Some(b)=>b,None=>"local"};
const MAX_OTA_BYTES:usize=24*1024*1024;
const MAX_LOG_BYTES:usize=128*1024;
fn dir()->PathBuf{std::env::var_os("AA_SETTINGS_DIR").map(PathBuf::from).unwrap_or_else(||"/persist/aa-settings".into())}
fn diag_dir()->PathBuf{std::env::var_os("AA_DIAG_DIR").map(PathBuf::from).unwrap_or_else(||"/var/lib/carfluxoid".into())}
fn bridge_dir()->PathBuf{std::env::var_os("AA_BRIDGE_DIR").map(PathBuf::from).unwrap_or_else(||"/run/carfluxoid".into())}
pub fn bluetooth_name()->String{
 std::fs::read_to_string(dir().join("bluetooth-name.txt")).ok()
  .map(|s|s.trim().to_owned()).filter(|s|valid_name(s)).unwrap_or_else(||"CarFluxoid".into())
}
pub fn oem_name()->String{
 std::fs::read_to_string(dir().join("name.txt")).ok().map(|s|s.trim().to_owned())
  .filter(|s|valid_name(s)).unwrap_or_else(||"Car".into())
}
pub fn logo()->(Vec<u8>, &'static str){
 let path=dir().join("logo.png");
 let source=match std::fs::read(&path){
  Ok(png) if decode_logo(&png).is_some()=>return (png,"custom"),
  Ok(_)=>"invalid",
  Err(error) if error.kind()==std::io::ErrorKind::NotFound=>"missing",
  Err(error)=>{eprintln!("CP_OEM_LOGO_READ {path:?}: {error}");"unreadable"}
 };
 (include_bytes!("../assets/car.png").to_vec(),source)
}
fn decode_logo(data:&[u8])->Option<image::DynamicImage>{
 dimensions(data)?;
 image::load_from_memory_with_format(data,image::ImageFormat::Png).ok()
}
pub fn projection_icons(logo:&[u8])->Vec<(u32,Vec<u8>)>{
 let Some(source)=decode_logo(logo) else {return Vec::new();};
 [120,180,256].into_iter().filter_map(|size|{
  let normalized=source.resize_exact(size,size,image::imageops::FilterType::Lanczos3).into_rgba8();
  let mut png=std::io::Cursor::new(Vec::new());
  image::DynamicImage::ImageRgba8(normalized).write_to(&mut png,image::ImageFormat::Png).ok()?;
  Some((size,png.into_inner()))
 }).collect()
}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum BarPosition { Left, Right, Bottom }
impl BarPosition {
 fn as_str(self)->&'static str{
  match self {Self::Left=>"left",Self::Right=>"right",Self::Bottom=>"bottom"}
 }
}
#[derive(Clone,Debug)]
pub struct Options {pub scale_percent:u32,pub max_resolution:u32,pub sidebar:BarPosition,pub assist_buttons:bool,pub return_button:bool,pub touch:bool,pub fps:u8,pub theme:String,
 /// Pixels hidden by the head unit at the top/bottom of the AA video; None follows the AA margin.
 pub inset_top:Option<u16>,pub inset_bottom:Option<u16>,
 /// Read by aa-phone: rewrite the iPhone H.264 stream to POC type 2 for the Uconnect VPU.
 pub h264_poc2:bool,
 /// Automatically start CarPlay when the car head unit connects.
 pub auto_start:bool}
impl Default for Options {fn default()->Self{Self{scale_percent:100,max_resolution:0,sidebar:BarPosition::Left,assist_buttons:true,return_button:true,touch:true,fps:30,theme:"auto".into(),inset_top:None,inset_bottom:None,h264_poc2:true,auto_start:true}}}
fn parse_inset(v:&str)->Result<Option<u16>,()>{if v=="auto"{return Ok(None);}let n:u16=v.parse().map_err(|_|())?;if n>200{return Err(());}Ok(Some(n))}
fn inset_json(v:Option<u16>)->String{v.map_or("auto".into(),|n|n.to_string())}
impl Options{pub fn physical_size(&self,w:u16,h:u16)->(u32,u32){let width=16000/self.scale_percent;(width,(width*h as u32/w.max(1) as u32).max(1))}}
fn parse_options(text:&str)->Result<Options,()>{let mut o=Options::default();for line in text.lines(){if line.trim().is_empty(){continue;}let (key,value)=line.split_once('=').ok_or(())?;match(key.trim(),value.trim()){
 ("scale_percent",v) if ["75","85","100","115","125","150"].contains(&v)=>o.scale_percent=v.parse().map_err(|_|())?,
 ("max_resolution",v) if ["0","480","720","1080"].contains(&v)=>o.max_resolution=v.parse().map_err(|_|())?,
 ("sidebar","left")=>o.sidebar=BarPosition::Left,
 ("sidebar","right")=>o.sidebar=BarPosition::Right,
 ("sidebar","bottom")=>o.sidebar=BarPosition::Bottom,
 ("return_button","true")=>o.return_button=true,("return_button","false")=>o.return_button=false,
 ("touch","true")=>o.touch=true,("touch","false")=>o.touch=false,
 ("assist_buttons","true")=>o.assist_buttons=true,("assist_buttons","false")=>o.assist_buttons=false,
 ("fps",v) if ["10","15","20","30"].contains(&v)=>o.fps=v.parse().map_err(|_|())?,
 ("theme",v) if ["auto","day","night"].contains(&v)=>o.theme=v.into(),
 ("inset_top",v)=>o.inset_top=parse_inset(v)?,("inset_bottom",v)=>o.inset_bottom=parse_inset(v)?,
 ("h264_poc2","1")=>o.h264_poc2=true,("h264_poc2","0")=>o.h264_poc2=false,
 ("auto_start","true")|("auto_start","1")=>o.auto_start=true,("auto_start","false")|("auto_start","0")=>o.auto_start=false,_=>return Err(())
 }}Ok(o)}
pub fn options()->Options{std::fs::read_to_string(dir().join("options.txt")).ok().and_then(|s|parse_options(&s).ok()).unwrap_or_default()}
fn options_json()->String{let o=options();format!(r#"{{"sidebar":"{}","return_button":{},"touch":true,"fps":{},"theme":"{}","assist_buttons":{},"max_resolution":{},"scale_percent":{},"inset_top":"{}","inset_bottom":"{}","h264_poc2":{},"auto_start":{}}}"#,o.sidebar.as_str(),o.return_button,o.fps,o.theme,o.assist_buttons,o.max_resolution,o.scale_percent,inset_json(o.inset_top),inset_json(o.inset_bottom),o.h264_poc2,o.auto_start)}
fn dimensions(b:&[u8])->Option<(u32,u32)>{
 if b.len()<33||b.len()>128*1024||&b[..8]!=b"\x89PNG\r\n\x1a\n"||&b[12..16]!=b"IHDR"{return None;}
 let w=u32::from_be_bytes(b[16..20].try_into().ok()?);let h=u32::from_be_bytes(b[20..24].try_into().ok()?);
 if w!=h||!(64..=512).contains(&w){return None;}Some((w,h))
}
fn valid_name(s:&str)->bool{!s.trim().is_empty()&&s.chars().count()<=32&&!s.chars().any(char::is_control)}
fn save(name:&str,data:&[u8])->std::io::Result<()>{
 use std::os::unix::fs::{OpenOptionsExt,PermissionsExt};
 let d=dir();match std::fs::create_dir(&d){Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>{},Err(e)=>return Err(e)};std::fs::set_permissions(&d,std::fs::Permissions::from_mode(0o700))?;
 let temp=d.join(format!(".{name}.tmp"));
 let result=(||{let mut f=std::fs::OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&temp)?;f.write_all(data)?;f.sync_all()?;std::fs::rename(&temp,d.join(name))})();
 if result.is_err(){let _=std::fs::remove_file(temp);}result
}
fn json_escape(s:&str)->String{
 let mut out=String::with_capacity(s.len()+4);
 for c in s.chars(){
  match c {
   '"'=>out.push_str("\\\""),
   '\\'=>out.push_str("\\\\"),
   '\n'=>out.push_str("\\n"),
   '\r'=>out.push_str("\\r"),
   '\t'=>out.push_str("\\t"),
   c if (c as u32)<0x20=>out.push_str(&format!("\\u{:04x}",c as u32)),
   c=>out.push(c),
  }
 }
 out
}
fn read_trimmed(path:impl AsRef<Path>)->Option<String>{
 std::fs::read_to_string(path).ok().map(|s|s.trim().to_string()).filter(|s|!s.is_empty())
}
fn read_tail(path:impl AsRef<Path>,max_bytes:usize)->Option<String>{
 let data=std::fs::read(path).ok()?;
 let slice=if data.len()>max_bytes {&data[data.len()-max_bytes..]} else {&data[..]};
 Some(String::from_utf8_lossy(slice).into_owned())
}
fn service_state(name:&str)->String{
 if let Some(state)=Command::new("systemctl").args(["is-active",name]).output().ok()
  .map(|o|String::from_utf8_lossy(&o.stdout).trim().to_string())
  .filter(|s|!s.is_empty()) {
  return state;
 }
 if let Some(pid)=read_trimmed(format!("/run/carfluxoid-svc/{name}.pid"))
  .and_then(|s|s.parse::<u32>().ok()) {
  if Path::new(&format!("/proc/{pid}")).exists() { return "active".into(); }
  return "inactive".into();
 }
 "unknown".into()
}
fn os_slot()->&'static str{
 match read_trimmed("/proc/cmdline").as_deref() {
  Some(cmd) if cmd.contains("root=/dev/mmcblk0p2")=>"Slot A (p2)",
  Some(cmd) if cmd.contains("root=/dev/mmcblk0p3")=>"Slot B (p3)",
  _=>"Standard",
 }
}
fn diag_json()->String{
 let temp_c=read_trimmed("/sys/class/thermal/thermal_zone0/temp")
  .and_then(|s|s.parse::<f64>().ok()).map(|m|format!("{:.1}",m/1000.0)).unwrap_or_else(||"n/a".into());
 let throttled=Command::new("vcgencmd").arg("get_throttled").output().ok()
  .filter(|o|o.status.success())
  .map(|o|String::from_utf8_lossy(&o.stdout).trim().trim_start_matches("throttled=").to_string())
  .filter(|s|!s.is_empty()).unwrap_or_else(||"n/a".into());
 let uptime_s=read_trimmed("/proc/uptime").and_then(|s|s.split_whitespace().next().and_then(|v|v.parse::<f64>().ok())).map(|v|v as u64).unwrap_or(0);
 let bridge=bridge_dir();
 let display=read_trimmed(bridge.join("video.sock.display")).map(|s|{
  let p:Vec<&str>=s.split_whitespace().collect();
  if p.len()>=4 {format!("{}x{} (marges {}x{})",p[0],p[1],p[2],p[3])} else {s}
 }).unwrap_or_else(||"en attente autoradio".into());
 let night_mode=match read_trimmed(bridge.join("video.sock.night")).as_deref() {
  Some("1")=>"nuit",Some("0")=>"jour",_=>"inconnu"
 };
 let bt_mac=read_trimmed(bridge.join("video.sock.btmac")).unwrap_or_else(||"—".into());
 let gps=read_trimmed(bridge.join("video.sock.gps")).unwrap_or_else(||"aucun fix".into());
 let gps_carplay=read_trimmed(bridge.join("video.sock.gps_cp")).unwrap_or_else(||"non demandé".into());
 let guidance=read_trimmed(bridge.join("video.sock.guidance.fmt")).map(|s|{
  let p:Vec<&str>=s.split_whitespace().collect();
  if p.len()>=2 {format!("{} Hz {}",p[0],if p[1]=="1"{"mono"}else{"stéréo"})} else {s}
 }).unwrap_or_else(||"inactif".into());
 let carplay_active=bridge.join("video.sock.touch").exists();
 let airplay_active=bridge.join("video.sock.airplay").exists();
 let usb_online=read_trimmed("/sys/module/aa_bulk/parameters/usb_online").unwrap_or_else(||"—".into());
 let accessory=read_trimmed("/sys/module/aa_bulk/parameters/accessory_active").unwrap_or_else(||"—".into());
 let rx_bytes=read_trimmed("/sys/module/aa_bulk/parameters/rx_bytes").unwrap_or_else(||"0".into());
 let tx_bytes=read_trimmed("/sys/module/aa_bulk/parameters/tx_bytes").unwrap_or_else(||"0".into());
 format!(
  r#"{{"version":"{}","build":"{}","slot":"{}","temp_c":"{}","throttled":"{}","uptime_s":{},"display":"{}","night_mode":"{}","bt_mac":"{}","gps":"{}","gps_carplay":"{}","guidance":"{}","carplay_active":{},"airplay_active":{},"usb_online":"{}","accessory_active":"{}","rx_bytes":"{}","tx_bytes":"{}","services":{{"aa":"{}","radio":"{}","dashboard":"{}","diag":"{}"}}}}"#,
  json_escape(APP_VERSION),json_escape(APP_BUILD),json_escape(os_slot()),
  json_escape(&temp_c),json_escape(&throttled),uptime_s,json_escape(&display),night_mode,
  json_escape(&bt_mac),json_escape(&gps),json_escape(&gps_carplay),json_escape(&guidance),carplay_active,airplay_active,
  json_escape(&usb_online),json_escape(&accessory),json_escape(&rx_bytes),json_escape(&tx_bytes),
  json_escape(&service_state("carfluxoid-aa")),json_escape(&service_state("carfluxoid-radio")),
  json_escape(&service_state("carfluxoid-dashboard")),json_escape(&service_state("carfluxoid-car-diag")),
 )
}
fn read_logs(query:&str)->String{
 let which=query.split('&').find_map(|p|p.strip_prefix("which=")).unwrap_or("prev");
 let (label,path)=match which {
  "current"=>("car-diag.log",diag_dir().join("car-diag.log")),
  "session"=>("session.log",bridge_dir().join("session.log")),
  _=>("car-diag.prev.log",diag_dir().join("car-diag.prev.log")),
 };
 read_tail(&path,MAX_LOG_BYTES).unwrap_or_else(||format!("Aucun log disponible pour {label} ({}).\n",path.display()))
}
fn validate_tar_entries(listing:&str)->Result<(),&'static str>{
 let mut count=0usize;
 for raw in listing.lines(){
  let entry=raw.trim();
  if entry.is_empty(){continue;}
  count+=1;
  if entry.starts_with('/')||entry.split('/').any(|part|part==".."){return Err("Archive rejetée : chemin non autorisé.");}
 }
 if count==0{return Err("Archive vide.");}
 Ok(())
}
fn handle_ota(body:&[u8])->Result<String,String>{
 if body.len()<32||body[0]!=0x1f||body[1]!=0x8b{
  return Err("Fichier invalide : sélectionnez une archive .tar.gz CarFluxoid.".into());
 }
 let stamp=SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0);
 let stage=PathBuf::from(format!("/tmp/carfluxoid-ota-{}-{stamp}",std::process::id()));
 std::fs::create_dir_all(&stage).map_err(|e|format!("Création dossier temporaire impossible : {e}"))?;
 let result=(||->Result<(PathBuf,PathBuf),String>{
  let archive=stage.join("bundle.tar.gz");
  std::fs::write(&archive,body).map_err(|e|format!("Écriture archive impossible : {e}"))?;
  let list=Command::new("tar").args(["-tzf"]).arg(&archive).output().map_err(|e|format!("tar indisponible : {e}"))?;
  if !list.status.success(){return Err("Archive .tar.gz corrompue ou illisible.".into());}
  validate_tar_entries(&String::from_utf8_lossy(&list.stdout)).map_err(|e|e.to_string())?;
  let extract_dir=stage.join("extracted");
  std::fs::create_dir_all(&extract_dir).map_err(|e|format!("Création dossier extraction impossible : {e}"))?;
  let ex=Command::new("tar").args(["-xzf"]).arg(&archive).args(["--no-same-owner","--no-same-permissions","-C"]).arg(&extract_dir).output()
   .map_err(|e|format!("Extraction impossible : {e}"))?;
  if !ex.status.success(){return Err(format!("Échec extraction : {}",String::from_utf8_lossy(&ex.stderr).trim()));}
  let bundle_dir=if extract_dir.join("install-dashboard.sh").is_file() {
   extract_dir
  }else if extract_dir.join("update-bundle/install-dashboard.sh").is_file() {
   extract_dir.join("update-bundle")
  }else{
   return Err("install-dashboard.sh introuvable dans l'archive.".into());
  };
  if !bundle_dir.join("SHA256SUMS").is_file(){return Err("SHA256SUMS introuvable dans l'archive.".into());}
  let chk=Command::new("sha256sum").args(["-c","SHA256SUMS"]).current_dir(&bundle_dir).output()
   .map_err(|e|format!("sha256sum indisponible : {e}"))?;
  if !chk.status.success(){
   return Err(format!("Vérification SHA256 échouée :\n{}",String::from_utf8_lossy(&chk.stdout).trim()));
  }
  let bin=bundle_dir.join("catplay-aa-rx");
  if bin.is_file(){
   use std::os::unix::fs::PermissionsExt;
   let _=std::fs::set_permissions(&bin,std::fs::Permissions::from_mode(0o755));
   let st=Command::new(&bin).arg("--self-test").output().map_err(|e|format!("Auto-test binaire impossible : {e}"))?;
   if !st.status.success(){return Err(format!("Auto-test binaire échoué (architecture incompatible ?) : {}",String::from_utf8_lossy(&st.stderr).trim()));}
  }
  Ok((stage.clone(),bundle_dir))
 })();
 match result {
  Err(err)=>{let _=std::fs::remove_dir_all(&stage);Err(err)}
  Ok((stage_dir,bundle_dir))=>{
   let script=bundle_dir.join("install-dashboard.sh");
   let cmd=format!("sleep 1; sh '{}' > /tmp/carfluxoid-ota.log 2>&1; rm -rf '{}'",script.display(),stage_dir.display());
   Command::new("sh").args(["-c",&format!("nohup sh -c \"{cmd}\" >/dev/null 2>&1 &")])
    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
    .spawn().map_err(|e|{let _=std::fs::remove_dir_all(&stage_dir);format!("Lancement installateur impossible : {e}")})?;
   Ok("Mise à jour vérifiée (SHA256 + auto-test OK). Installation et redémarrage en cours (patientez ~15 s)…".into())
  }
 }
}
fn reply(s:&mut TcpStream,code:&str,mime:&str,data:&[u8]){
 let header=format!("HTTP/1.1 {code}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",data.len());let _=s.write_all(header.as_bytes());let _=s.write_all(data);
}
fn handle(mut s:TcpStream)->std::io::Result<()>{
 s.set_read_timeout(Some(Duration::from_secs(15)))?;s.set_write_timeout(Some(Duration::from_secs(15)))?;
 let mut data=Vec::new();let mut block=[0;4096];let end;
 loop{let n=s.read(&mut block)?;if n==0{return Ok(());}data.extend_from_slice(&block[..n]);if let Some(i)=data.windows(4).position(|w|w==b"\r\n\r\n"){end=i+4;break;}if data.len()>8192{return Ok(());}}
 let header=String::from_utf8_lossy(&data[..end]).to_string();let mut first=header.lines().next().unwrap_or("").split_whitespace();let method=first.next().unwrap_or("");let raw_target=first.next().unwrap_or("");let (path,query)=raw_target.split_once('?').unwrap_or((raw_target,""));
 if method!="GET" {
  let origin=header.lines().filter_map(|l|l.split_once(':')).find(|(k,_)|k.eq_ignore_ascii_case("origin")).map(|(_,v)|v.trim());
  let host=header.lines().filter_map(|l|l.split_once(':')).find(|(k,_)|k.eq_ignore_ascii_case("host")).map(|(_,v)|v.trim()).unwrap_or("");
  if origin.is_some_and(|o|o!=format!("http://{host}")){reply(&mut s,"403 Forbidden","text/plain",b"Origin refused");return Ok(());}
 }
 let size=header.lines().filter_map(|l|l.split_once(':')).find(|(k,_)|k.eq_ignore_ascii_case("content-length")).and_then(|(_,v)|v.trim().parse::<usize>().ok()).unwrap_or(0);
 let is_ota=method=="POST"&&path=="/ota";
 let max_size=if is_ota {MAX_OTA_BYTES} else {128*1024};
 if size>max_size{reply(&mut s,"413 Payload Too Large","text/plain",if is_ota {b"Archive OTA: 24 Mio maximum"} else {b"PNG: 128 Kio maximum"});return Ok(());}
 if is_ota{let _=s.set_read_timeout(Some(Duration::from_secs(120)));}
 while data.len()-end<size{let n=s.read(&mut block)?;if n==0{return Ok(());}data.extend_from_slice(&block[..n]);}
 let body=&data[end..end+size];
 match(method,path){
  ("GET","/health")=>reply(&mut s,"200 OK","text/plain",b"CarFluxoid settings ready"),
  ("GET","/")=>reply(&mut s,"200 OK","text/html; charset=utf-8",HTML.as_bytes()),
  ("GET","/name")=>reply(&mut s,"200 OK","text/plain; charset=utf-8",oem_name().as_bytes()),
  ("GET","/options")=>reply(&mut s,"200 OK","application/json",options_json().as_bytes()),
  ("GET","/diag")=>reply(&mut s,"200 OK","application/json; charset=utf-8",diag_json().as_bytes()),
  ("GET","/logs")=>reply(&mut s,"200 OK","text/plain; charset=utf-8",read_logs(query).as_bytes()),
  ("POST","/ota")=>{
   match handle_ota(body){
    Ok(msg)=>reply(&mut s,"200 OK","text/plain; charset=utf-8",msg.as_bytes()),
    Err(err)=>reply(&mut s,"400 Bad Request","text/plain; charset=utf-8",err.as_bytes()),
   }
  },
  ("PUT","/options")=>{
   if body.len()>1024||std::str::from_utf8(body).ok().and_then(|s|parse_options(s).ok()).is_none(){reply(&mut s,"400 Bad Request","text/plain",b"Options invalides");}
   else{match save("options.txt",body){Ok(())=>reply(&mut s,"200 OK","text/plain",b"OK"),Err(e)=>{eprintln!("WEB_SETTINGS_SAVE options: {e}");reply(&mut s,"507 Insufficient Storage","text/plain",b"Enregistrement impossible")}}}
  },
  ("GET","/logo.png")=>reply(&mut s,"200 OK","image/png",&logo().0),
  ("PUT","/name")|("PUT","/logo.png")=>{
   let valid=if path=="/name"{std::str::from_utf8(body).map(valid_name).unwrap_or(false)}else{decode_logo(body).is_some()};
   if !valid{reply(&mut s,"400 Bad Request","text/plain; charset=utf-8",b"Nom: 1 a 32 caracteres. Logo: PNG carre 64 a 512 pixels, 128 Kio maximum.");}
   else{match save(if path=="/name"{"name.txt"}else{"logo.png"},body){Ok(())=>reply(&mut s,"200 OK","text/plain",b"OK"),Err(e)=>{eprintln!("WEB_SETTINGS_SAVE {path}: {e}");reply(&mut s,"507 Insufficient Storage","text/plain",b"Ecriture impossible; ancien reglage conserve")}}}
  },
  ("POST","/reset")=>{let _=std::fs::remove_file(dir().join("logo.png"));let _=std::fs::remove_file(dir().join("name.txt"));let _=std::fs::remove_file(dir().join("options.txt"));reply(&mut s,"200 OK","text/plain",b"OK");},
  _=>reply(&mut s,"404 Not Found","text/plain",b"Not found")
 }Ok(())
}
pub fn serve()->std::io::Result<()>{let listener=TcpListener::bind("0.0.0.0:80")?;eprintln!("WEB_SETTINGS_READY http://192.168.50.2");for stream in listener.incoming(){match stream{Ok(s)=>{if let Err(e)=handle(s){eprintln!("WEB_SETTINGS_REQUEST: {e}");}},Err(e)=>{eprintln!("WEB_SETTINGS_ACCEPT: {e}");std::thread::sleep(Duration::from_millis(100));}}}Ok(())}
#[cfg(test)]mod tests{use super::*;#[test]fn png_and_names(){assert!(valid_name("Peugeot"));assert!(!valid_name("\n"));assert!(!valid_name(&"x".repeat(33)));assert_eq!(dimensions(include_bytes!("../assets/car.png")),Some((256,256)));assert!(dimensions(b"bad").is_none());}#[test]fn tar_path_guard_rejects_traversal(){assert!(validate_tar_entries("update-bundle/aa-phone\nupdate-bundle/SHA256SUMS\n").is_ok());assert!(validate_tar_entries("../etc/passwd\n").is_err());assert!(validate_tar_entries("/etc/passwd\n").is_err());assert!(validate_tar_entries("   \n").is_err());}}

#[cfg(test)]mod scale_tests{use super::*;#[test]fn scale_preserves_default_and_aspect(){for(percent,width)in[(75,213),(100,160),(125,128),(150,106)]{let o=parse_options(&format!("scale_percent={percent}")).unwrap();assert_eq!(o.physical_size(800,480),(width,width*480/800));}assert!(parse_options("scale_percent=0").is_err());assert!(parse_options("scale_percent=999").is_err());}}

#[cfg(test)]
#[test]
fn custom_logo_is_rendered_at_carplay_sizes() {
 let mut source=std::io::Cursor::new(Vec::new());
 image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(128,128,image::Rgba([13,73,185,255])))
  .write_to(&mut source,image::ImageFormat::Png).unwrap();
 let variants=projection_icons(source.get_ref());
 assert_eq!(variants.iter().map(|(size,_)|*size).collect::<Vec<_>>(),[120,180,256]);
 for (size,png) in variants{
  let decoded=image::load_from_memory_with_format(&png,image::ImageFormat::Png).unwrap().into_rgba8();
  assert_eq!(decoded.dimensions(),(size,size));
  assert_eq!(decoded.get_pixel(size/2,size/2).0,[13,73,185,255]);
 }
}
