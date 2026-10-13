#!/usr/bin/env python3
"""
fake_govee.py - pretend to be one or more Govee LAN lights, for testing
ReadySetGlow without hardware.

Each fake light answers multicast scans (239.255.255.250:4001), accepts
commands on <its ip>:4003, replies to devStatus, and decodes ptReal / razer
packets so you can see exactly what the app sends.

  python3 tools/fake_govee.py 127.0.0.2:H619A 127.0.0.3:H6008

On Linux every 127.x.y.z address is local, so several fakes can share a PC.
Prints one line per command; --stats prints a per-light summary instead.
"""
import argparse, base64, json, socket, struct, sys, threading, time

MCAST = "239.255.255.250"


def xor(b):
    x = 0
    for v in b:
        x ^= v
    return x


def decode_pt(pkt):
    if len(pkt) != 20:
        return f"BAD LEN {len(pkt)}"
    if xor(pkt[:19]) != pkt[19]:
        return "BAD CHECKSUM"
    if pkt[:4] == bytes([0x33, 0x05, 0x15, 0x01]):
        mask = pkt[12] | (pkt[13] << 8)
        segs = [i + 1 for i in range(15) if mask >> i & 1]
        return f"seg {segs} = #{pkt[4]:02x}{pkt[5]:02x}{pkt[6]:02x}"
    if pkt[:3] == bytes([0x33, 0x05, 0x04]):
        return f"scene {pkt[3]:#04x}"
    return pkt[:6].hex(" ")


def decode_razer(pkt):
    if pkt[0] != 0xBB or xor(pkt[:-1]) != pkt[-1] or pkt[2] != len(pkt) - 5:
        return f"BAD razer {pkt.hex(' ')}"
    if pkt[3] == 0xB1:
        return f"razer {'ON' if pkt[4] else 'OFF'}"
    if pkt[3] == 0xB0:
        n = pkt[5]
        cols = [pkt[6 + 3 * i: 9 + 3 * i].hex() for i in range(n)]
        return f"razer frame grad={pkt[4]} n={n} first={cols[:3]}"
    return pkt.hex(" ")


class Light:
    def __init__(self, ip, sku, quiet, stats):
        self.ip, self.sku, self.quiet, self.stats = ip, sku, quiet, stats
        self.state = {"onOff": 1, "brightness": 100, "color": {"r": 255, "g": 255, "b": 255}, "colorTemInKelvin": 0}
        self.counts = {}
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind((ip, 4003))
        threading.Thread(target=self.loop, daemon=True).start()

    def scan_reply(self):
        return {"msg": {"cmd": "scan", "data": {
            "ip": self.ip, "device": "AA:BB:CC:DD:EE:" + self.ip.split(".")[-1].zfill(2), "sku": self.sku,
            "bleVersionHard": "3.01.01", "bleVersionSoft": "1.03.01", "wifiVersionHard": "1.00.10", "wifiVersionSoft": "1.02.03"}}}

    def loop(self):
        while True:
            data, addr = self.sock.recvfrom(65535)
            try:
                m = json.loads(data)["msg"]
            except Exception as e:
                print(f"[{self.ip}] BAD JSON {e}: {data[:80]!r}", flush=True)
                continue
            cmd, d = m["cmd"], m.get("data", {})
            self.counts[cmd] = self.counts.get(cmd, 0) + 1
            desc = ""
            if cmd == "turn":
                self.state["onOff"] = d["value"]; desc = f"power {d['value']}"
            elif cmd == "brightness":
                self.state["brightness"] = d["value"]; desc = f"brightness {d['value']}"
            elif cmd == "colorwc":
                self.state["color"] = d["color"]; self.state["colorTemInKelvin"] = d["colorTemInKelvin"]
                c = d["color"]; desc = f"colour #{c['r']:02x}{c['g']:02x}{c['b']:02x} K={d['colorTemInKelvin']}"
            elif cmd == "devStatus":
                self.sock.sendto(json.dumps({"msg": {"cmd": "devStatus", "data": self.state}}).encode(), (addr[0], 4002))
                desc = "status requested"
            elif cmd == "ptReal":
                desc = "; ".join(decode_pt(base64.b64decode(p)) for p in d["command"])
            elif cmd == "razer":
                desc = decode_razer(base64.b64decode(d["pt"]))
            else:
                desc = json.dumps(d)
            if not self.quiet and not self.stats:
                print(f"[{self.ip} {self.sku}] {cmd}: {desc}", flush=True)
            if "BAD" in desc:
                print(f"[{self.ip}] !!! {desc}", flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("lights", nargs="+", help="ip:SKU")
    ap.add_argument("--quiet", action="store_true")
    ap.add_argument("--stats", type=float, default=0, help="print per-light command counts every N seconds")
    a = ap.parse_args()
    lights = [Light(*spec.split(":"), a.quiet, a.stats) for spec in a.lights]

    ms = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    ms.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    ms.bind(("", 4001))
    mreq = struct.pack("4s4s", socket.inet_aton(MCAST), socket.inet_aton("0.0.0.0"))
    try:
        ms.setsockopt(socket.IPPROTO_IP, socket.IP_ADD_MEMBERSHIP, mreq)
    except OSError as e:
        print("multicast join failed:", e, flush=True)

    def scan_loop():
        while True:
            data, addr = ms.recvfrom(4096)
            if b'"scan"' in data:
                print(f"scan from {addr}", flush=True)
                for l in lights:
                    l.sock.sendto(json.dumps(l.scan_reply()).encode(), (addr[0], 4002))
    threading.Thread(target=scan_loop, daemon=True).start()
    print("fake lights up:", ", ".join(f"{l.ip} {l.sku}" for l in lights), flush=True)
    while True:
        time.sleep(a.stats or 3600)
        if a.stats:
            print(json.dumps({l.ip: l.counts for l in lights}), flush=True)


if __name__ == "__main__":
    main()
