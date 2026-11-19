use std::{io::{Read,Write},net::{TcpListener,TcpStream},path::PathBuf,time::Duration};
const HTML:&str=include_str!("../assets/settings.html");
fn dir()->PathBuf{std::env::var_os("AA_SETTINGS_DIR").map(PathBuf::from).unwrap_or_else(||"/persist/aa-settings".into())}
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
 /// Read by aa-phone-v821: rewrite the iPhone H.264 stream to POC type 2 for the Uconnect VPU.
 pub h264_poc2:bool}
impl Default for Options {fn default()->Self{Self{scale_percent:100,max_resolution:0,sidebar:BarPosition::Left,assist_buttons:true,return_button:true,touch:true,fps:30,theme:"auto".into(),inset_top:None,inset_bottom:None,h264_poc2:true}}}
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
 ("h264_poc2","1")=>o.h264_poc2=true,("h264_poc2","0")=>o.h264_poc2=false,_=>return Err(())
 }}Ok(o)}
pub fn options()->Options{std::fs::read_to_string(dir().join("options.txt")).ok().and_then(|s|parse_options(&s).ok()).unwrap_or_default()}
fn options_json()->String{let o=options();format!(r#"{{"sidebar":"{}","return_button":{},"touch":true,"fps":{},"theme":"{}","assist_buttons":{},"max_resolution":{},"scale_percent":{},"inset_top":"{}","inset_bottom":"{}","h264_poc2":{}}}"#,o.sidebar.as_str(),o.return_button,o.fps,o.theme,o.assist_buttons,o.max_resolution,o.scale_percent,inset_json(o.inset_top),inset_json(o.inset_bottom),o.h264_poc2)}
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
fn reply(s:&mut TcpStream,code:&str,mime:&str,data:&[u8]){
 let header=format!("HTTP/1.1 {code}\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",data.len());let _=s.write_all(header.as_bytes());let _=s.write_all(data);
}
fn handle(mut s:TcpStream)->std::io::Result<()>{
 s.set_read_timeout(Some(Duration::from_secs(15)))?;s.set_write_timeout(Some(Duration::from_secs(15)))?;
 let mut data=Vec::new();let mut block=[0;4096];let end;
 loop{let n=s.read(&mut block)?;if n==0{return Ok(());}data.extend_from_slice(&block[..n]);if let Some(i)=data.windows(4).position(|w|w==b"\r\n\r\n"){end=i+4;break;}if data.len()>8192{return Ok(());}}
 let header=String::from_utf8_lossy(&data[..end]).to_string();let mut first=header.lines().next().unwrap_or("").split_whitespace();let method=first.next().unwrap_or("");let path=first.next().unwrap_or("").split('?').next().unwrap_or("");
 if method!="GET" {
  let origin=header.lines().filter_map(|l|l.split_once(':')).find(|(k,_)|k.eq_ignore_ascii_case("origin")).map(|(_,v)|v.trim());
  let host=header.lines().filter_map(|l|l.split_once(':')).find(|(k,_)|k.eq_ignore_ascii_case("host")).map(|(_,v)|v.trim()).unwrap_or("");
  if origin.is_some_and(|o|o!=format!("http://{host}")){reply(&mut s,"403 Forbidden","text/plain",b"Origin refused");return Ok(());}
 }
 let size=header.lines().filter_map(|l|l.split_once(':')).find(|(k,_)|k.eq_ignore_ascii_case("content-length")).and_then(|(_,v)|v.trim().parse::<usize>().ok()).unwrap_or(0);
 if size>128*1024{reply(&mut s,"413 Payload Too Large","text/plain",b"PNG: 128 Kio maximum");return Ok(());}
 while data.len()-end<size{let n=s.read(&mut block)?;if n==0{return Ok(());}data.extend_from_slice(&block[..n]);}
 let body=&data[end..end+size];
 match(method,path){
  ("GET","/health")=>reply(&mut s,"200 OK","text/plain",b"CarFluxoid settings ready"),
  ("GET","/")=>reply(&mut s,"200 OK","text/html; charset=utf-8",HTML.as_bytes()),
  ("GET","/name")=>reply(&mut s,"200 OK","text/plain; charset=utf-8",oem_name().as_bytes()),
  ("GET","/options")=>reply(&mut s,"200 OK","application/json",options_json().as_bytes()),
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
#[cfg(test)]mod tests{use super::*;#[test]fn png_and_names(){assert!(valid_name("Peugeot"));assert!(!valid_name("\n"));assert!(!valid_name(&"x".repeat(33)));assert_eq!(dimensions(include_bytes!("../assets/car.png")),Some((256,256)));assert!(dimensions(b"bad").is_none());}}

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
