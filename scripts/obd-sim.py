#!/usr/bin/env python3
"""
CarFluxoid Interactive Virtual OBD-II Simulator (Fiat Tipo MultiJet)
--------------------------------------------------------------------
Drives the real 800x480 CarFluxoid Uconnect Slint preview window (`dashboard-ui --preview`)
AND exposes a local CarFluxoid-compatible HTTP API (http://0.0.0.0:8088/api/obd/live)
so you can test both the Uconnect OBD Sport screen and the iPhone app from your Mac!

Controls (keyboard or sliders):
  UP / Z / W   : Accelerator (hold)
  DOWN / S     : Brake (hold)
  E / Q / 0-6  : Shift gear up/down (switches to Manual mode)
  A            : Toggle Auto / Manual gearbox
  R            : Toggle FAP Regeneration alert (620°C)
  D            : Toggle Autopilot demo cruise
"""

import http.server
import json
import math
import os
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import tkinter as tk
from pathlib import Path

PREVIEW_DIR = Path(tempfile.gettempdir()) / "carfluxoid-preview-settings"
OBD_JSON = PREVIEW_DIR / "obd.json"
DISPLAY_FILE = PREVIEW_DIR / "video.sock.display"
HTTP_PORT = 8088

GEAR_RATIOS = {
    1: 8.5,   # km/h per 1000 rpm
    2: 15.5,
    3: 24.0,
    4: 33.5,
    5: 43.5,
    6: 54.0,
}


class MultiJetSim:
    def __init__(self):
        self.lock = threading.Lock()
        self.throttle_cmd = 0.0    # 0..100
        self.brake_cmd = 0.0       # 0..100
        self.auto_gearbox = True
        self.autopilot = False
        self.dpf_regen = False
        self.margins_32 = True

        self.speed_kmh = 0.0
        self.rpm = 820.0
        self.gear = 0              # 0=N, 1..6
        self.boost_bar = -0.04
        self.coolant_c = 88.0
        self.fuel_pct = 64.0
        self.dpf_temp_c = 285.0
        self.dpf_soot_pct = 42.0
        self.battery_v = 14.3
        self.dist_since_clear = 14820
        self.start_time = time.time()
        self.trip_dist_km = 0.0
        self.trip_fuel_l = 0.0
        self.lat = 45.7640
        self.lon = 4.8357
        self.heading = 0.6

    def step(self, dt: float):
        with self.lock:
            now = time.time()
            if self.autopilot:
                phase = (now - self.start_time) * 0.35
                self.throttle_cmd = max(0.0, min(100.0, 42.0 + 42.0 * math.sin(phase)))
                self.brake_cmd = max(0.0, -35.0 * math.sin(phase))

            thr = self.throttle_cmd / 100.0
            brk = self.brake_cmd / 100.0

            # Auto gearbox logic
            if self.auto_gearbox:
                if self.speed_kmh < 1.5 and thr < 0.05:
                    self.gear = 0
                else:
                    if self.gear == 0:
                        self.gear = 1
                    ratio = GEAR_RATIOS[self.gear]
                    est_rpm = self.speed_kmh / ratio * 1000.0
                    up_rpm = 2300 + thr * 1400
                    down_rpm = 1350
                    if est_rpm > up_rpm and self.gear < 6:
                        self.gear += 1
                    elif est_rpm < down_rpm and self.gear > 1:
                        self.gear -= 1

            # Longitudinal acceleration (1.6 MultiJet ~120ch / 320 Nm)
            gear_mult = {0: 0.0, 1: 3.1, 2: 2.0, 3: 1.35, 4: 0.95, 5: 0.72, 6: 0.55}[self.gear]
            accel = thr * 5.8 * gear_mult - brk * 24.0 - 0.12 - (self.speed_kmh ** 2) * 0.00018
            if self.gear == 0:
                accel = -brk * 24.0 - 0.25
            self.speed_kmh = max(0.0, min(205.0, self.speed_kmh + accel * dt * 3.6))

            # RPM dynamics
            if self.gear == 0:
                target_rpm = 820.0 + thr * 3400.0
            else:
                wheel_rpm = (self.speed_kmh / GEAR_RATIOS[self.gear]) * 1000.0
                target_rpm = max(820.0, min(4850.0, wheel_rpm))
            self.rpm += (target_rpm - self.rpm) * min(1.0, dt * 8.0)

            # Turbo boost spool lag
            load = max(8.0, min(100.0, 12.0 + thr * 88.0 * (min(self.rpm, 2600.0) / 2200.0)))
            target_boost = -0.05 + (load / 100.0) * 1.55 * min(1.0, max(0.15, (self.rpm - 900.0) / 1400.0))
            self.boost_bar += (target_boost - self.boost_bar) * min(1.0, dt * 3.8)

            # Wideband Lambda (0134) + MAF (0110) + exact Diesel consumption
            map_kpa = 101.3 + self.boost_bar * 100.0
            maf_gps = max(6.5, (map_kpa * 1.598 / (8.3145 * 300.0)) * (self.rpm / 120.0) * 0.88 * 28.97)
            if thr < 0.02 and self.speed_kmh > 18.0 and self.gear > 0:
                lam = 8.80  # overrun fuel cut -> 0.0 L/100!
                lph = 0.0
            else:
                lam = max(1.18, min(5.8, 5.2 - (load / 100.0) * 3.9))
                afr = 14.5 * lam
                lph = (maf_gps / afr) * 3.6 / 0.835

            l100 = round(min(45.0, (lph / self.speed_kmh) * 100.0), 1) if self.speed_kmh >= 5.0 else None
            kw = min(125.0, max(0.0, lph * 3.58))
            power_hp = round(kw * 1.3596)
            torque_nm = round(min(360.0, (kw * 9549.3) / max(self.rpm, 800.0)))

            # EGR closes under high load
            egr_pct = round(max(0.0, min(85.0, 58.0 - load * 0.62)), 1)
            egr_err = round(math.sin(now * 1.3) * 1.8, 1)
            rail_bar = round(310.0 + (load / 100.0) * 1280.0)

            # DPF temperature
            target_dpf = 625.0 if self.dpf_regen else (210.0 + load * 2.1)
            self.dpf_temp_c += (target_dpf - self.dpf_temp_c) * min(1.0, dt * 1.6)
            if self.dpf_regen and self.dpf_soot_pct > 6.0:
                self.dpf_soot_pct = max(6.0, self.dpf_soot_pct - dt * 0.4)

            # Trip integration + simulated GPS movement
            step_km = self.speed_kmh * (dt / 3600.0)
            self.trip_dist_km += step_km
            self.trip_fuel_l += lph * (dt / 3600.0)
            self.fuel_pct = max(0.0, self.fuel_pct - (lph * (dt / 3600.0) / 50.0) * 100.0)
            if self.speed_kmh > 1.0:
                self.heading += 0.03 * math.sin(now * 0.25) * dt
                dlat = (step_km / 111.0) * math.cos(self.heading)
                dlon = (step_km / (111.0 * math.cos(math.radians(self.lat)))) * math.sin(self.heading)
                self.lat += dlat
                self.lon += dlon

            runtime_s = int(now - self.start_time)
            return {
                "connected": True,
                "state": "connected",
                "adapter_mac": "00:1D:A5:04:51:44",
                "adapter_name": "IOS-Vlink (Simulateur)",
                "transport": "ble",
                "elm_version": "ELM327 v2.3 (Vgate Sim)",
                "protocol": "AUTO, ISO 15765-4 (CAN 11/500)",
                "supported_pids": [
                    "0101", "0104", "0105", "010B", "010C", "010D", "010F", "0110",
                    "011F", "0123", "012C", "012D", "012F", "0130", "0131", "0133",
                    "0134", "0142", "0149",
                ],
                "rpm": round(self.rpm),
                "speed_kmh": round(self.speed_kmh, 1),
                "coolant_c": round(self.coolant_c),
                "intake_c": 26.0,
                "load_pct": round(load, 1),
                "throttle_pct": round(self.throttle_cmd, 1),
                "map_kpa": round(map_kpa, 1),
                "baro_kpa": 101.3,
                "boost_bar": round(self.boost_bar, 2),
                "maf_gps": round(maf_gps, 1),
                "rail_bar": rail_bar,
                "fuel_level_pct": round(self.fuel_pct, 1),
                "fuel_rate_lph": round(lph, 2),
                "fuel_l_per_100km": l100,
                "battery_v": round(self.battery_v, 2),
                "egr_pct": egr_pct,
                "egr_error_pct": egr_err,
                "lambda": round(lam, 2),
                "pedal_pct": round(self.throttle_cmd, 1),
                "dpf_temp_c": round(self.dpf_temp_c),
                "dpf_soot_pct": round(self.dpf_soot_pct, 1),
                "dpf_regen_active": self.dpf_temp_c >= 480.0,
                "gear": "N" if self.gear == 0 else str(self.gear),
                "power_hp": power_hp,
                "torque_nm": torque_nm,
                "runtime_s": runtime_s,
                "dist_since_clear_km": self.dist_since_clear + int(self.trip_dist_km),
                "warmups_since_clear": 255,
                "vin": "ZFA3560000SIM1234",
                "gps": {
                    "lat": round(self.lat, 6),
                    "lon": round(self.lon, 6),
                    "alt_m": 175.0,
                    "speed_kmh": round(self.speed_kmh, 1),
                    "bearing": round(math.degrees(self.heading) % 360, 1),
                    "ts": int(now),
                },
                "dtcs": [],
                "debug_log": [
                    "Simulateur Fiat Tipo 1.6 MultiJet actif",
                    f"RPM={round(self.rpm)} V={round(self.speed_kmh)}km/h Boost={self.boost_bar:+.2f}bar λ={lam:.2f}",
                ],
                "updated_at": int(now),
            }


def write_atomic(path: Path, text: str):
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(text, encoding="utf-8")
    tmp.replace(path)


def local_ip() -> str:
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("8.8.8.8", 80))
        ip = s.getsockname()[0]
        s.close()
        return ip
    except Exception:
        return "127.0.0.1"


def start_http_server(get_snapshot):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            pass

        def _send_json(self, obj, code=200):
            data = json.dumps(obj).encode("utf-8")
            self.send_response(code)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path == "/api/obd/live":
                self._send_json(get_snapshot())
            elif self.path == "/diag":
                self._send_json({
                    "version": "1.3.1-sim",
                    "build": "mac-sim",
                    "slot": "A",
                    "temp_c": "38.2 °C",
                    "uptime_s": 120,
                    "display": "800x480 (marges 0x32)",
                    "night_mode": "nuit",
                    "gps": "Simulateur actif",
                    "gps_carplay": "actif",
                    "guidance": "inactif",
                    "carplay_active": False,
                    "airplay_active": False,
                })
            elif self.path == "/api/trips":
                self._send_json([])
            elif self.path == "/api/obd/devices":
                self._send_json([])
            else:
                self.send_response(404)
                self.end_headers()

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            if length > 0:
                self.rfile.read(length)
            if self.path == "/api/time":
                self._send_json({"unix": int(time.time())})
            else:
                self._send_json(get_snapshot())

    class ReuseServer(socketserver.ThreadingTCPServer):
        allow_reuse_address = True

    srv = ReuseServer(("0.0.0.0", HTTP_PORT), Handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def main():
    PREVIEW_DIR.mkdir(parents=True, exist_ok=True)
    write_atomic(DISPLAY_FILE, "800 480 0 32\n")

    sim = MultiJetSim()
    latest = [sim.step(0.05)]
    write_atomic(OBD_JSON, json.dumps(latest[0]))

    # Launch the real native 800x480 CarFluxoid Slint Uconnect window if --no-ui isn't passed.
    repo_root = Path(__file__).resolve().parent.parent
    ui_proc = None
    if "--no-ui" not in sys.argv:
        cmd = [
            "cargo", "run",
            "--manifest-path", str(repo_root / "dashboard-ui" / "Cargo.toml"),
            "--features", "preview",
            "--", "--preview",
        ]
        print("Lancement de la fenêtre CarFluxoid Uconnect 800x480 (Slint)…")
        ui_proc = subprocess.Popen(cmd, cwd=str(repo_root))

    start_http_server(lambda: latest[0])
    ip = local_ip()

    # Build interactive Tkinter driving console
    root = tk.Tk()
    root.title("CarFluxoid — Simulateur OBD-II Fiat Tipo MultiJet")
    root.configure(bg="#090e18")
    root.geometry("460x590")

    def lbl(parent, text, size=11, bold=False, fg="#f4f7fc"):
        weight = "bold" if bold else "normal"
        return tk.Label(parent, text=text, bg="#090e18", fg=fg, font=("SF Pro Display", size, weight))

    lbl(root, "Simulateur OBD-II · Fiat Tipo MultiJet", 16, True, "#14d4b4").pack(pady=(12, 2))
    lbl(root, f"API iPhone optionnelle : http://{ip}:{HTTP_PORT}", 10, False, "#8ea2be").pack(pady=(0, 8))

    readout_var = tk.StringVar(value="0 km/h · 820 tr/min · Rapport N")
    tk.Label(
        root, textvariable=readout_var, bg="#121c2e", fg="#f4f7fc",
        font=("Menlo", 13, "bold"), padx=12, pady=10, relief="flat"
    ).pack(fill="x", padx=16, pady=6)

    sub_var = tk.StringVar(value="Boost +0.00 bar · 0 ch · λ 5.0 · 0.0 L/100")
    lbl(root, "", 10, False, "#38bdf8").pack_forget()
    tk.Label(
        root, textvariable=sub_var, bg="#090e18", fg="#38bdf8",
        font=("Menlo", 11, "bold")
    ).pack(pady=(0, 8))

    # Sliders
    def make_slider(title, from_, to_, init, cmd):
        frame = tk.Frame(root, bg="#090e18")
        frame.pack(fill="x", padx=18, pady=4)
        lbl(frame, title, 10, True, "#8ea2be").pack(anchor="w")
        s = tk.Scale(
            frame, from_=from_, to=to_, orient="horizontal",
            bg="#121c2e", fg="#f4f7fc", troughcolor="#1d2c46",
            activebackground="#14d4b4", highlightthickness=0, command=cmd
        )
        s.set(init)
        s.pack(fill="x")
        return s

    thr_slider = make_slider("Accélérateur (%) — Maintiens ↑ ou Z", 0, 100, 0,
                             lambda v: setattr(sim, "throttle_cmd", float(v)))
    brk_slider = make_slider("Frein (%) — Maintiens ↓ ou S", 0, 100, 0,
                             lambda v: setattr(sim, "brake_cmd", float(v)))
    fuel_slider = make_slider("Niveau carburant PID 012F (%) — Monte de +20% pour tester auto-plein", 0, 100, 64,
                              lambda v: setattr(sim, "fuel_pct", float(v)))
    cool_slider = make_slider("Température eau (°C)", 20, 110, 88,
                              lambda v: setattr(sim, "coolant_c", float(v)))

    # Buttons row
    btns = tk.Frame(root, bg="#090e18")
    btns.pack(fill="x", padx=18, pady=10)

    auto_var = tk.BooleanVar(value=True)
    demo_var = tk.BooleanVar(value=False)
    fap_var = tk.BooleanVar(value=False)
    margin_var = tk.BooleanVar(value=True)

    def toggle_margins():
        sim.margins_32 = margin_var.get()
        write_atomic(DISPLAY_FILE, "800 480 0 32\n" if sim.margins_32 else "800 480 0 0\n")

    tk.Checkbutton(
        btns, text="Boîte Auto", variable=auto_var,
        command=lambda: setattr(sim, "auto_gearbox", auto_var.get()),
        bg="#090e18", fg="#f4f7fc", selectcolor="#121c2e"
    ).grid(row=0, column=0, sticky="w", padx=4)
    tk.Checkbutton(
        btns, text="Pilote Démo (D)", variable=demo_var,
        command=lambda: setattr(sim, "autopilot", demo_var.get()),
        bg="#090e18", fg="#14d4b4", selectcolor="#121c2e"
    ).grid(row=0, column=1, sticky="w", padx=4)
    tk.Checkbutton(
        btns, text="Régén. FAP 620°C (R)", variable=fap_var,
        command=lambda: setattr(sim, "dpf_regen", fap_var.get()),
        bg="#090e18", fg="#f59e0b", selectcolor="#121c2e"
    ).grid(row=1, column=0, sticky="w", padx=4, pady=4)
    tk.Checkbutton(
        btns, text="Marges Uconnect 32px", variable=margin_var,
        command=toggle_margins,
        bg="#090e18", fg="#38bdf8", selectcolor="#121c2e"
    ).grid(row=1, column=1, sticky="w", padx=4, pady=4)

    # Gear buttons
    gframe = tk.Frame(root, bg="#090e18")
    gframe.pack(pady=6)
    lbl(gframe, "Boîte manuelle :", 10, True, "#8ea2be").pack(side="left", padx=(0, 6))
    for g in range(7):
        def set_g(val=g):
            sim.auto_gearbox = False
            auto_var.set(False)
            sim.gear = val
        tk.Button(
            gframe, text="N" if g == 0 else str(g), width=3,
            command=set_g, bg="#172338", fg="#f4f7fc", relief="flat"
        ).pack(side="left", padx=2)

    lbl(root, "Clavier : ↑/Z Accélérer · ↓/S Freiner · E/Q Rapports · R FAP · D Démo", 9, False, "#647896").pack(pady=(10, 6))

    keys_held = set()

    def on_key_down(ev):
        k = ev.keysym.lower()
        keys_held.add(k)
        if k == "e":
            sim.auto_gearbox = False
            auto_var.set(False)
            sim.gear = min(6, sim.gear + 1)
        elif k == "q":
            sim.auto_gearbox = False
            auto_var.set(False)
            sim.gear = max(0, sim.gear - 1)
        elif k in ("0", "1", "2", "3", "4", "5", "6"):
            sim.auto_gearbox = False
            auto_var.set(False)
            sim.gear = int(k)
        elif k == "a":
            auto_var.set(not auto_var.get())
            sim.auto_gearbox = auto_var.get()
        elif k == "r":
            fap_var.set(not fap_var.get())
            sim.dpf_regen = fap_var.get()
        elif k == "d":
            demo_var.set(not demo_var.get())
            sim.autopilot = demo_var.get()

    def on_key_up(ev):
        keys_held.discard(ev.keysym.lower())

    root.bind("<KeyPress>", on_key_down)
    root.bind("<KeyRelease>", on_key_up)

    def tick():
        if not sim.autopilot:
            if any(k in keys_held for k in ("up", "z", "w")):
                sim.throttle_cmd = min(100.0, sim.throttle_cmd + 9.0)
                thr_slider.set(int(sim.throttle_cmd))
            elif thr_slider.get() > 0 and "button1" not in keys_held:
                # Gently release accelerator when keyboard isn't held and slider was driven by keys
                pass
            if any(k in keys_held for k in ("down", "s")):
                sim.brake_cmd = min(100.0, sim.brake_cmd + 15.0)
                brk_slider.set(int(sim.brake_cmd))
            elif sim.brake_cmd > 0 and not any(k in keys_held for k in ("down", "s")):
                sim.brake_cmd = max(0.0, sim.brake_cmd - 18.0)
                brk_slider.set(int(sim.brake_cmd))

        snap = sim.step(0.08)
        latest[0] = snap
        write_atomic(OBD_JSON, json.dumps(snap))

        readout_var.set(f"{snap['speed_kmh']:5.1f} km/h · {snap['rpm']:4d} tr/min · V{snap['gear']}")
        cons = f"{snap['fuel_l_per_100km']:.1f} L/100" if snap["fuel_l_per_100km"] is not None else f"{snap['fuel_rate_lph']:.1f} L/h"
        sub_var.set(f"Boost {snap['boost_bar']:+.2f} bar · {snap['power_hp']} ch · λ {snap['lambda']:.2f} · {cons}")
        root.after(80, tick)

    def on_close():
        if ui_proc and ui_proc.poll() is None:
            ui_proc.terminate()
        root.destroy()

    root.protocol("WM_DELETE_WINDOW", on_close)
    root.after(80, tick)
    root.mainloop()


if __name__ == "__main__":
    main()
