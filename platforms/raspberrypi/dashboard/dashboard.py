#!/usr/bin/env python3
"""Render the projected dashboard on demand and handle Android Auto touch input."""
import argparse
import os
import re
import socket
import struct
import subprocess
import tempfile
import time
from functools import lru_cache
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont, ImageOps

SIZES = {(800, 480), (1280, 720), (1920, 1080)}
FONT_DIR = Path("/usr/share/fonts/truetype/dejavu")
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9 ._-]{0,31}")
INK, MUTED, ACCENT = "#f2f6ff", "#b3c0d5", "#79edd2"
WAIT_BACK = (28, 377, 266, 439)


@lru_cache(maxsize=24)
def font(pixels, bold):
    filename = "DejaVuSans-Bold.ttf" if bold else "DejaVuSans.ttf"
    path = FONT_DIR / filename
    if not path.is_file():
        path = Path("/System/Library/Fonts/Supplemental") / ("Arial Bold.ttf" if bold else "Arial.ttf")
    return ImageFont.truetype(str(path), pixels) if path.is_file() else ImageFont.load_default(size=pixels)


def read(path, fallback=""):
    try:
        return path.read_text().strip()
    except OSError:
        return fallback


def replace_file(path, data, durable=False):
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as temp:
        temporary = Path(temp.name)
        try:
            temp.write(data)
            temp.flush()
            if durable:
                os.fsync(temp.fileno())
            os.replace(temporary, path)
            if durable:
                directory = os.open(path.parent, os.O_RDONLY)
                try:
                    os.fsync(directory)
                finally:
                    os.close(directory)
        finally:
            temporary.unlink(missing_ok=True)


def encode(image, source, target, converter):
    """Match the existing waiting screen decoder configuration, including SPS/PPS."""
    from importlib.util import module_from_spec, spec_from_file_location

    spec = spec_from_file_location("waiting_iphone_config", converter)
    module = module_from_spec(spec)
    spec.loader.exec_module(module)
    nals = list(module.nals(source.read_bytes()))
    sps, pps = (next(nal for nal in nals if nal and nal[0] & 31 == kind) for kind in (7, 8))
    config = b"\0\0\0\1" + sps + b"\0\0\0\1" + pps
    chroma = module.pps_params(pps)["chroma"]
    with tempfile.TemporaryDirectory(dir=target.parent) as folder:
        raw, aligned = Path(folder) / "raw.h264", Path(folder) / "aligned.h264"
        subprocess.run([
            "ffmpeg", "-hide_banner", "-loglevel", "error", "-f", "rawvideo",
            "-pixel_format", "rgb24", "-video_size", f"{image.width}x{image.height}",
            "-framerate", "30", "-i", "pipe:0", "-vf", "tpad=stop_mode=clone:stop=1",
            "-frames:v", "2", "-an", "-pix_fmt", "yuv420p", "-c:v", "libx264",
            "-preset", "ultrafast", "-crf", "17", "-profile:v", "high", "-threads", "2",
            "-x264-params", f"keyint=1:bframes=0:cabac=1:8x8dct=1:chroma-qp-offset={chroma}:deblock=0,0",
            "-f", "h264", str(raw),
        ], input=image.tobytes(), check=True, timeout=8)
        module.main(raw, config.hex(), aligned)
        replace_file(target, aligned.read_bytes())


class Dashboard:
    def __init__(self, settings, assets, runtime, bridge):
        self.settings, self.assets, self.runtime, self.bridge = settings, assets, runtime, bridge
        self.page = "home"
        self.draft = ""
        self.upper = False
        self.notice = ""
        self.buttons = []
        self.size = (800, 480)
        self.pressed = None
        self.waiting_picture = None
        self.carplay_job = None
        self.auto_started_key = None
        self.auto_start_pending = None
        self.control = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)

    def dimensions(self):
        try:
            values = read(self.bridge / "video.sock.display").split()
            size = (int(values[0]), int(values[1]))
            return size if size in SIZES else (800, 480)
        except (IndexError, ValueError):
            return (800, 480)

    def option(self, key, fallback):
        entries = dict(line.split("=", 1) for line in read(self.settings / "options.txt").splitlines() if "=" in line)
        return entries.get(key, fallback)

    def cycle(self, key, values):
        current = self.option(key, values[0])
        index = values.index(current) if current in values else 0
        lines = read(self.settings / "options.txt").splitlines()
        lines = [line for line in lines if line.split("=", 1)[0].strip() != key]
        lines.append(f"{key}={values[(index+1)%len(values)]}")
        replace_file(self.settings / "options.txt", ("\n".join(lines)+"\n").encode(), durable=True)
        self.notice = "Redémarre CarPlay pour appliquer les réglages."

    def shell_action(self, value):
        try:
            self.control.sendto(bytes([value]), str(self.bridge / "video.sock.control"))
        except OSError:
            self.notice = "Connexion à l’autoradio en cours"

    def start_carplay(self, restart=False):
        command = ["/opt/carfluxoid/bin/start-ap-trial.sh"]
        if restart:
            command.append("--restart")
        try:
            if self.carplay_job is None or self.carplay_job.poll() is not None:
                self.carplay_job = subprocess.Popen(command, stdin=subprocess.DEVNULL)
        except OSError as error:
            print(f"DASHBOARD_CARPLAY_START_FAILED {error}", flush=True)
            self.notice = "Impossible de lancer le Wi-Fi CarFluxoid."
            return
        self.page = "home"
        self.shell_action(7 if restart else 5)

    def display_session_key(self):
        display_file = self.bridge / "video.sock.display"
        control_file = self.bridge / "video.sock.control"
        try:
            st = display_file.stat()
        except OSError:
            return None
        if not control_file.exists():
            return None
        return (st.st_ino, st.st_mtime_ns)

    def maybe_auto_start(self):
        if self.option("auto_start", "true") not in {"true", "1"}:
            return False
        key = self.display_session_key()
        if key is None:
            self.auto_start_pending = None
            return False
        if self.auto_started_key == key:
            return False
        now = time.monotonic()
        if self.auto_start_pending is None or self.auto_start_pending[0] != key:
            self.auto_start_pending = (key, now)
            return False
        # Wait 2 s so the head-unit projection link settles before Wi-Fi AP/BLE startup.
        if now - self.auto_start_pending[1] < 2.0:
            return False
        self.auto_started_key = key
        self.auto_start_pending = None
        print("DASHBOARD_AUTO_START head_unit=connected", flush=True)
        self.start_carplay(restart=False)
        # Do not force a redundant ffmpeg re-encode while start-ap-trial.sh starts the radios.
        width, height = self.dimensions()
        return not (self.runtime / f"dashboard-wait-{width}x{height}.h264").is_file()

    def check_carplay_job(self):
        if self.carplay_job is None:
            return False
        status = self.carplay_job.poll()
        if status is None:
            return False
        self.carplay_job = None
        if status == 0:
            return False
        print(f"DASHBOARD_CARPLAY_START_FAILED exit={status}", flush=True)
        self.shell_action(8)
        self.notice = "Le Wi-Fi ou le récepteur CarPlay n’a pas démarré."
        return True

    def choose(self, action):
        self.notice = ""
        if self.auto_start_pending is not None:
            # User interacted with the dashboard before auto-start fired: stay in the menu.
            self.auto_started_key = self.auto_start_pending[0]
            self.auto_start_pending = None
        if action in {"home", "settings", "network"}:
            self.page = action
        elif action == "resume":
            self.start_carplay()
        elif action == "wait_back":
            self.page = "home"
            self.shell_action(8)
        elif action == "radio":
            self.shell_action(6)
        elif action == "restart":
            self.start_carplay(restart=True)
        elif action == "auto_start":
            self.cycle("auto_start", ["true", "false"])
        elif action == "sidebar":
            self.cycle("sidebar", ["left", "right", "bottom"])
        elif action == "theme":
            self.cycle("theme", ["auto", "day", "night"])
        elif action == "fps":
            self.cycle("fps", ["30", "20", "15", "10"])
        elif action in {"wifi", "bluetooth"}:
            self.page = action
            self.upper = False
            self.draft = self.radio_name(action)
        elif action == "shift":
            self.upper = not self.upper
        elif action == "erase":
            self.draft = self.draft[:-1]
        elif action == "clear":
            self.draft = ""
        elif action == "save":
            name = self.draft.strip()
            if not NAME.fullmatch(name):
                self.notice = "Nom invalide · 1 à 32 caractères"
                return
            replace_file(self.settings / f"{self.page}-name.txt", (name+"\n").encode(), durable=True)
            self.page = "network"
            self.notice = "Nom enregistré · actif au prochain essai"
        elif action.startswith("key:") and len(self.draft) < 32:
            self.draft += action[4:]

    def radio_name(self, kind):
        default = "CarFluxoid-Pi_AP" if kind == "wifi" else "CarFluxoid"
        return read(self.settings / f"{kind}-name.txt", default)

    def render(self, waiting=False):
        # The grid shares one coordinate system with the touch hit targets.
        size = self.dimensions()
        sx, sy = size[0]/800, size[1]/480
        image = Image.new("RGB", size, "#0e141f")
        draw = ImageDraw.Draw(image)
        buttons = []
        page = "waiting" if waiting else self.page

        def rect(box):
            x1,y1,x2,y2=box
            return round(x1*sx),round(y1*sy),round(x2*sx),round(y2*sy)

        def text(x, y, value, px=20, color=INK, bold=False, anchor=None, limit=None):
            face=font(round(px*sy), bold)
            if limit is not None and face.getlength(value)>limit*sx:
                while value and face.getlength(value+"…")>limit*sx:
                    value=value[:-1]
                value+="…"
            draw.text((round(x*sx),round(y*sy)), value, fill=color, font=face, anchor=anchor)

        def panel(box, fill="#192333", border="#2c3b51", radius=16):
            draw.rounded_rectangle(rect(box), round(radius*sy), fill=fill, outline=border, width=max(1, round(sy)))

        def icon(kind, x, y, color=ACCENT):
            width=max(2,round(2.4*sy))
            def line(*points):
                draw.line([(round((x+a)*sx),round((y+b)*sy)) for a,b in points], fill=color, width=width, joint="curve")
            if kind in {"next","back"}:
                direction=1 if kind=="next" else -1
                line((-3*direction,-6),(3*direction,0),(-3*direction,6))
            elif kind == "settings":
                for offset, notch in [(-6,-3),(0,4),(6,-1)]:
                    line((-10,offset),(10,offset))
                    line((notch,offset-2),(notch,offset+2))
            elif kind == "network":
                for radius in (5,10,15):
                    draw.arc(rect((x-radius,y+5-radius,x+radius,y+5+radius)),222,318,fill=color,width=width)
                draw.ellipse(rect((x-1.6,y+4,x+1.6,y+7.2)),fill=color)
            elif kind == "restart":
                draw.arc(rect((x-9,y-9,x+9,y+9)),-44,270,fill=color,width=width)
                line((-4,-13),(0,-9),(-4,-5))
            elif kind == "radio":
                line((7,-8),(7,8),(-5,8))
                line((-1,-6),(-7,0),(-1,6))
                line((-7,0),(11,0))

        def button(box, label, action, accent=False, secondary=None):
            panel(box, "#22554f" if accent else "#192333", "#458d80" if accent else "#2c3b51")
            x1,y1,x2,y2=box
            if secondary:
                text(x1+19,y1+15,label,21,bold=True)
                text(x1+19,y2-29,secondary,16,MUTED)
                icon("next",x2-26,(y1+y2)/2,MUTED)
            else:
                text((x1+x2)/2,(y1+y2)/2,label,19,bold=True,anchor="mm")
            buttons.append((box,action))

        title = {"home":"CarFluxoid","settings":"Réglages CarPlay","network":"Wi-Fi et Bluetooth",
                 "wifi":"Nom du Wi-Fi","bluetooth":"Nom Bluetooth","waiting":"Démarrer CarPlay"}[page]
        text(28,25,title,34,bold=True)
        draw.line(rect((28,83,772,83)),fill="#2c3b51",width=max(1,round(sy)))
        if waiting:
            panel((28,108,772,345),"#172e34","#305455",21)
            text(54,130,"Connecte ton iPhone",39,bold=True)
            text(54,201,"Dans les réglages Bluetooth, sélectionne",20,MUTED)
            text(54,239,self.radio_name("bluetooth"),22,ACCENT,True,limit=668)
            text(54,290,"Accepte ensuite la demande CarPlay sur l’iPhone.",18,MUTED)
            button(WAIT_BACK,"←  Retour au menu","wait_back")
        elif page == "home":
            logo = self.settings / "logo.png"
            try:
                with Image.open(logo) as source:
                    if max(source.size)>512:
                        raise ValueError("oversize icon")
                    artwork = ImageOps.contain(source.convert("RGBA"), (round(48*sx),round(48*sy)))
                    image.paste(artwork, (round(724*sx),round(20*sy)), artwork)
            except (OSError, ValueError):
                pass
            panel((28,103,451,347),"#172e34","#305455",21)
            text(52,122,"CarPlay",43,bold=True)
            text(53,193,"Connecter ou reprendre l’iPhone",19,MUTED)
            button((51,270,428,326),"Démarrer CarPlay","resume",True)
            for y, label, action, glyph in [
                (103,"Réglages CarPlay","settings","settings"),
                (188,"Wi-Fi et Bluetooth","network","network"),
                (273,"Redémarrer CarPlay","restart","restart"),
            ]:
                box=(471,y,772,y+74)
                panel(box)
                icon(glyph,499,y+37)
                text(526,y+37,label,18,bold=True,anchor="lm")
                buttons.append((box,action))
            panel((28,368,772,427))
            icon("radio",55,397)
            text(80,397,"Retour à l’autoradio",20,bold=True,anchor="lm")
            icon("next",745,397,MUTED)
            buttons.append(((28,368,772,427),"radio"))
            if not self.notice:
                text(28,446,"Réglages web   192.168.50.2",16,MUTED)
        elif page == "settings":
            bar={"left":"Gauche","right":"Droite","bottom":"En bas"}
            theme={"auto":"Automatique","day":"Clair","night":"Sombre"}
            auto={"true":"Activé","1":"Activé","false":"Désactivé","0":"Désactivé"}
            button((30,102,770,164),"Démarrage auto CarPlay","auto_start",secondary=auto.get(self.option("auto_start","true"),"Activé"))
            button((30,172,770,234),"Position de la barre","sidebar",secondary=bar.get(self.option("sidebar","left"),"Gauche"))
            button((30,242,770,304),"Apparence","theme",secondary=theme.get(self.option("theme","auto"),"Automatique"))
            button((30,312,770,374),"Images par seconde","fps",secondary=self.option("fps","30"))
            button((30,390,170,440),"Retour","home")
            button((498,390,770,440),"Redémarrer CarPlay","restart",True)
        elif page == "network":
            button((30,120,770,204),"Nom du Wi-Fi","wifi",secondary=self.radio_name("wifi"))
            button((30,221,770,305),"Nom Bluetooth","bluetooth",secondary=self.radio_name("bluetooth"))
            button((30,386,170,439),"Retour","home")
        else:
            draw.rounded_rectangle(rect((30,110,770,162)),round(10*sy),fill="#172638",outline="#318f9b")
            text(45,121,self.draft or "Nouveau nom",22)
            for row, letters in enumerate(("1234567890","qwertyuiop","asdfghjkl","zxcvbnm._-")):
                for col,char in enumerate(letters.upper() if self.upper else letters):
                    x,y=30+col*74,175+row*49
                    button((x,y,x+67,y+43),char,"key:"+char)
            for x,width,label,action in [(30,95,"Retour","network"),(133,95,"Maj","shift"),(236,150,"Espace","key: "),
                                          (394,108,"Effacer","erase"),(510,106,"Vider","clear"),(624,146,"Enregistrer","save")]:
                button((x,382,x+width,436),label,action,action=="save")
        if not waiting:
            if self.notice:
                text(28,446,self.notice,15,ACCENT)
            self.buttons = buttons
            self.size = size
        return image

    def publish(self):
        image = self.render()
        width,height = self.size
        source = self.assets/f"attente-{width}x{height}.h264"
        converter = self.assets/"waiting_iphone_config.py"
        encode(image, source, self.runtime/f"dashboard-{width}x{height}.h264", converter)
        waiting_key = (self.size, self.radio_name("bluetooth"))
        waiting_path = self.runtime/f"dashboard-wait-{width}x{height}.h264"
        if self.waiting_picture != waiting_key or not waiting_path.is_file():
            encode(self.render(waiting=True), source, waiting_path, converter)
            self.waiting_picture = waiting_key

    def touch(self, report):
        if len(report) != 5:
            return False
        action,x,y=struct.unpack("<BHH",report)
        waiting=bool(action & 0x80)
        action &= 0x7f
        x,y=x*800/self.size[0],y*480/self.size[1]
        buttons=[(WAIT_BACK,"wait_back")] if waiting else self.buttons
        hit=next((label for (l,t,r,b),label in buttons if l<=x<r and t<=y<b),None)
        if action==0:
            self.pressed=hit
        if action==1:
            pressed,self.pressed=self.pressed,None
            if pressed and pressed==hit:
                self.choose(hit)
                return True
        return False


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument("--settings",type=Path,default=Path("/var/lib/carfluxoid/settings"))
    parser.add_argument("--assets",type=Path,default=Path("/opt/carfluxoid/assets"))
    parser.add_argument("--runtime",type=Path,default=Path("/run/carfluxoid-ui"))
    parser.add_argument("--bridge",type=Path,default=Path("/run/carfluxoid"))
    args=parser.parse_args()
    ui=Dashboard(args.settings,args.assets,args.runtime,args.bridge)
    endpoint=args.runtime/"input.sock"
    endpoint.unlink(missing_ok=True)
    with socket.socket(socket.AF_UNIX,socket.SOCK_DGRAM) as server:
        server.bind(str(endpoint))
        server.settimeout(0.5)
        ui.publish()
        while True:
            try:
                changed=ui.touch(server.recv(32))
            except TimeoutError:
                changed=False
            carplay_failed = ui.check_carplay_job()
            auto_started = ui.maybe_auto_start()
            if changed or carplay_failed or auto_started or ui.size!=ui.dimensions():
                ui.publish()


if __name__=="__main__":
    main()
