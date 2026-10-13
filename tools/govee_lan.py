#!/usr/bin/env python3
"""
govee_lan.py - minimal Govee LAN API client (stdlib only, no cloud).

Protocol:
  - Discovery: send a "scan" JSON to multicast 239.255.255.250:4001.
    Devices reply to UDP port 4002 on this machine.
  - Control: send JSON commands to <device_ip>:4003.
  - Status: send "devStatus" to <device_ip>:4003. The reply comes back on port 4002.

Prerequisite: in the Govee Home app, open each device's settings and toggle
"LAN Control" on. You only need to do this once per device. Not every model
supports it.

Usage:
  python govee_lan.py scan
  python govee_lan.py on   <ip>
  python govee_lan.py off  <ip>
  python govee_lan.py bright <ip> <1-100>
  python govee_lan.py color  <ip> <r> <g> <b>
  python govee_lan.py temp   <ip> <kelvin 2000-9000>
  python govee_lan.py status <ip>
  python govee_lan.py rainbow <ip> [seconds]

Windows: allow inbound UDP 4002 for Python in Windows Defender Firewall, or
scan and status will never receive a reply. If you have several network
adapters (VPN, Hyper-V, WSL), pass --iface <your LAN IP>.
"""
import argparse
import colorsys
import json
import socket
import struct
import time

MCAST_ADDR = "239.255.255.250"
SCAN_PORT = 4001
LISTEN_PORT = 4002
CMD_PORT = 4003


def _listener(timeout: float) -> socket.socket:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("", LISTEN_PORT))
    s.settimeout(timeout)
    return s


def scan(iface: str = "0.0.0.0", timeout: float = 3.0) -> list[dict]:
    rx = _listener(0.5)
    tx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM, socket.IPPROTO_UDP)
    tx.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 2)
    if iface != "0.0.0.0":
        tx.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_IF, socket.inet_aton(iface))
    msg = {"msg": {"cmd": "scan", "data": {"account_topic": "reserve"}}}
    tx.sendto(json.dumps(msg).encode(), (MCAST_ADDR, SCAN_PORT))

    found, deadline = {}, time.time() + timeout
    while time.time() < deadline:
        try:
            data, addr = rx.recvfrom(4096)
        except socket.timeout:
            continue
        try:
            d = json.loads(data)["msg"]["data"]
            found[d.get("ip", addr[0])] = d
        except (ValueError, KeyError):
            pass
    rx.close(); tx.close()
    return list(found.values())


def send(ip: str, cmd: str, data: dict) -> None:
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.sendto(json.dumps({"msg": {"cmd": cmd, "data": data}}).encode(), (ip, CMD_PORT))
    s.close()


def turn(ip, on: bool):            send(ip, "turn", {"value": 1 if on else 0})
def brightness(ip, pct: int):      send(ip, "brightness", {"value": max(1, min(100, pct))})
def color(ip, r, g, b):            send(ip, "colorwc", {"color": {"r": r, "g": g, "b": b}, "colorTemInKelvin": 0})
def color_temp(ip, k: int):        send(ip, "colorwc", {"color": {"r": 0, "g": 0, "b": 0}, "colorTemInKelvin": k})


def status(ip: str, timeout: float = 2.0) -> dict | None:
    rx = _listener(timeout)
    send(ip, "devStatus", {})
    try:
        while True:
            data, addr = rx.recvfrom(4096)
            if addr[0] == ip:
                return json.loads(data)["msg"]["data"]
    except socket.timeout:
        return None
    finally:
        rx.close()


def rainbow(ip: str, seconds: float = 20, fps: int = 10):
    # Keep the rate modest. Some models drop packets above ~10-20 Hz.
    t0 = time.time()
    while time.time() - t0 < seconds:
        h = ((time.time() - t0) / 6.0) % 1.0
        r, g, b = (int(c * 255) for c in colorsys.hsv_to_rgb(h, 1, 1))
        color(ip, r, g, b)
        time.sleep(1 / fps)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--iface", default="0.0.0.0", help="LAN IP to send multicast from")
    p.add_argument("cmd")
    p.add_argument("args", nargs="*")
    a = p.parse_args()

    if a.cmd == "scan":
        devs = scan(a.iface)
        if not devs:
            print("No devices replied. Check that LAN Control is on in the Govee app, "
                  "UDP 4002 is open in the firewall, and --iface is set if you have several adapters.")
        for d in devs:
            print(f"{d.get('ip'):15}  {d.get('sku'):8}  {d.get('device')}")
        return

    ip, rest = a.args[0], a.args[1:]
    match a.cmd:
        case "on":      turn(ip, True)
        case "off":     turn(ip, False)
        case "bright":  brightness(ip, int(rest[0]))
        case "color":   color(ip, *map(int, rest[:3]))
        case "temp":    color_temp(ip, int(rest[0]))
        case "status":  print(json.dumps(status(ip), indent=2))
        case "rainbow": rainbow(ip, float(rest[0]) if rest else 20)
        case _:         p.error(f"unknown command {a.cmd}")


if __name__ == "__main__":
    main()
