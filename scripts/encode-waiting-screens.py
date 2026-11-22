"""Encode the user-supplied waiting screens without regenerating their artwork."""
from pathlib import Path
from PIL import Image
import subprocess,shutil
HERE=Path(__file__).resolve().parents[1]/'assets/waiting';INPUT=HERE
for w,h in [(800,480),(1280,720),(1920,1080)]:
 src=INPUT/f'attente-{w}x{h}.png'
 with Image.open(src) as im:assert im.size==(w,h),(src,im.size)
 video=HERE/f'attente-{w}x{h}.h264'
 subprocess.run(['ffmpeg','-hide_banner','-loglevel','error','-y','-i',str(src),'-frames:v','1','-c:v','libx264','-profile:v','baseline','-pix_fmt','yuv420p','-crf','18','-preset','medium','-tune','stillimage','-x264-params','keyint=1:repeat-headers=1','-f','h264',str(video)],check=True)
 assert video.stat().st_size<=1024*1024
 r=subprocess.run(['ffprobe','-v','error','-select_streams','v:0','-show_entries','stream=width,height','-of','csv=p=0',str(video)],capture_output=True,text=True,check=True)
 assert r.stdout.strip()==f'{w},{h}',r.stdout
 print(src.name,'-> H264',video.stat().st_size,'octets')
