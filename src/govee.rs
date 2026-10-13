//! Govee LAN protocol: discovery, JSON commands and the binary "ptReal" and
//! "razer" sub-protocols used for per-segment and per-LED control.
//!
//! - Scan: JSON to multicast 239.255.255.250:4001, replies arrive on UDP 4002.
//! - Commands: JSON to <ip>:4003. `devStatus` replies arrive on 4002.
//! - ptReal: base64 20-byte BLE-style packets (`0x33 ...` + XOR checksum).
//! - razer: base64 `0xBB 0x00 len cmd data.. xor` packets for streaming
//!   (the protocol Razer Chroma / DreamView sync uses).

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde_json::{Value, json};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

pub const MCAST: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
pub const SCAN_PORT: u16 = 4001;
pub const LISTEN_PORT: u16 = 4002;
pub const CMD_PORT: u16 = 4003;

// ---------------------------------------------------------------------------
// Packet builders (pure functions, unit tested)
// ---------------------------------------------------------------------------

pub fn xor(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |a, b| a ^ b)
}

/// A 20-byte BLE-style packet: payload zero-padded to 19 bytes, then XOR.
pub fn ble_packet(payload: &[u8]) -> [u8; 20] {
    let mut p = [0u8; 20];
    let n = payload.len().min(19);
    p[..n].copy_from_slice(&payload[..n]);
    p[19] = xor(&p[..19]);
    p
}

/// Bitmask of segments (0-based indices) as two little-endian bytes.
pub fn segment_mask(segments: impl IntoIterator<Item = usize>) -> [u8; 2] {
    let mut m: u16 = 0;
    for s in segments {
        if s < 15 {
            m |= 1 << s;
        }
    }
    m.to_le_bytes()
}

/// Set an RGB colour on a set of segments: `33 05 15 01 RR GG BB 00x5 LL HH`.
pub fn segment_color_packet(rgb: [u8; 3], mask: [u8; 2]) -> [u8; 20] {
    ble_packet(&[
        0x33, 0x05, 0x15, 0x01, rgb[0], rgb[1], rgb[2], 0, 0, 0, 0, 0, mask[0], mask[1],
    ])
}

/// Built-in scene by code: `33 05 04 <code>`.
pub fn scene_packet(code: u8) -> [u8; 20] {
    ble_packet(&[0x33, 0x05, 0x04, code])
}

/// Razer-protocol packet: `BB 00 <len> <cmd> <data..> <xor>`.
pub fn razer_packet(cmd: u8, data: &[u8]) -> Vec<u8> {
    let len = data.len().min(255) as u8;
    let mut p = Vec::with_capacity(data.len() + 5);
    p.extend_from_slice(&[0xBB, 0x00, len, cmd]);
    p.extend_from_slice(&data[..len as usize]);
    p.push(xor(&p));
    p
}

pub fn razer_enable(on: bool) -> Vec<u8> {
    razer_packet(0xB1, &[on as u8])
}

/// Max colours per razer frame given the single length byte (2 + 3n <= 255).
pub const RAZER_MAX_LEDS: usize = 84;

pub fn razer_frame(colors: &[[u8; 3]], gradient: bool) -> Vec<u8> {
    let n = colors.len().min(RAZER_MAX_LEDS);
    let mut data = Vec::with_capacity(2 + n * 3);
    data.push(gradient as u8);
    data.push(n as u8);
    for c in &colors[..n] {
        data.extend_from_slice(c);
    }
    razer_packet(0xB0, &data)
}

/// Parse a hex string (spaces allowed). With `checksum`, the input is treated
/// as a BLE payload and padded/checksummed to 20 bytes.
pub fn parse_hex(s: &str, checksum: bool) -> Result<Vec<u8>, String> {
    let clean: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',')
        .collect();
    let clean = clean.trim_start_matches("0x");
    if !clean.len().is_multiple_of(2) {
        return Err("odd number of hex digits".into());
    }
    let bytes: Result<Vec<u8>, _> = (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16))
        .collect();
    let bytes = bytes.map_err(|e| e.to_string())?;
    Ok(if checksum {
        ble_packet(&bytes).to_vec()
    } else {
        bytes
    })
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

// JSON command bodies -------------------------------------------------------

pub fn msg(cmd: &str, data: Value) -> Value {
    json!({ "msg": { "cmd": cmd, "data": data } })
}

pub fn cmd_turn(on: bool) -> Value {
    msg("turn", json!({ "value": on as u8 }))
}

pub fn cmd_brightness(pct: u8) -> Value {
    msg("brightness", json!({ "value": pct.clamp(1, 100) }))
}

pub fn cmd_color(rgb: [u8; 3]) -> Value {
    msg(
        "colorwc",
        json!({ "color": { "r": rgb[0], "g": rgb[1], "b": rgb[2] }, "colorTemInKelvin": 0 }),
    )
}

pub fn cmd_kelvin(k: u32) -> Value {
    msg(
        "colorwc",
        json!({ "color": { "r": 0, "g": 0, "b": 0 }, "colorTemInKelvin": k }),
    )
}

pub fn cmd_status() -> Value {
    msg("devStatus", json!({}))
}

pub fn cmd_scan() -> Value {
    msg("scan", json!({ "account_topic": "reserve" }))
}

pub fn cmd_pt_real(packets: &[&[u8]]) -> Value {
    let list: Vec<String> = packets.iter().map(|p| B64.encode(p)).collect();
    msg("ptReal", json!({ "command": list }))
}

pub fn cmd_razer(packet: &[u8]) -> Value {
    msg("razer", json!({ "pt": B64.encode(packet) }))
}

// ---------------------------------------------------------------------------
// Networking
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Incoming {
    pub from: IpAddr,
    pub cmd: String,
    pub data: Value,
    pub raw: String,
}

/// One UDP socket bound to 4002, shared for sending and receiving.
#[derive(Clone)]
pub struct Net {
    sock: Arc<UdpSocket>,
    /// False if 4002 was taken (e.g. Govee Desktop running) and we fell back
    /// to a random port: commands still work, but scan/status replies won't arrive.
    pub listening: bool,
}

impl Net {
    pub fn open(iface: Option<Ipv4Addr>) -> std::io::Result<(Net, Receiver<Incoming>)> {
        let make = |port: u16| -> std::io::Result<Socket> {
            let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
            s.set_reuse_address(true)?;
            s.bind(&SockAddr::from(SocketAddr::from((
                Ipv4Addr::UNSPECIFIED,
                port,
            ))))?;
            Ok(s)
        };
        let (sock, listening) = match make(LISTEN_PORT) {
            Ok(s) => (s, true),
            Err(_) => (make(0)?, false),
        };
        sock.set_multicast_ttl_v4(2)?;
        if let Some(ip) = iface {
            let _ = sock.set_multicast_if_v4(&ip);
        }
        let _ = sock.join_multicast_v4(&MCAST, &iface.unwrap_or(Ipv4Addr::UNSPECIFIED));
        sock.set_read_timeout(Some(Duration::from_millis(500)))?;
        let sock: UdpSocket = sock.into();
        let sock = Arc::new(sock);

        let (tx, rx) = channel();
        let reader = sock.clone();
        std::thread::Builder::new()
            .name("govee-rx".into())
            .spawn(move || recv_loop(reader, tx))?;
        Ok((Net { sock, listening }, rx))
    }

    pub fn send(&self, ip: IpAddr, body: &Value) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(body).expect("json");
        self.sock.send_to(&bytes, (ip, CMD_PORT)).map(|_| ())
    }

    pub fn scan(&self) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&cmd_scan()).expect("json");
        self.sock.send_to(&bytes, (MCAST, SCAN_PORT))?;
        Ok(())
    }

    /// Unicast scan to one address. Useful when multicast is blocked (some
    /// mesh Wi-Fi and guest networks) but you know the light's IP.
    pub fn probe(&self, ip: IpAddr) -> std::io::Result<()> {
        let bytes = serde_json::to_vec(&cmd_scan()).expect("json");
        self.sock.send_to(&bytes, (ip, SCAN_PORT))?;
        self.send(ip, &cmd_status())
    }
}

fn recv_loop(sock: Arc<UdpSocket>, tx: Sender<Incoming>) {
    let mut buf = vec![0u8; 8192];
    loop {
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                let raw = String::from_utf8_lossy(&buf[..n]).to_string();
                let (cmd, data) = match serde_json::from_str::<Value>(&raw) {
                    Ok(v) => {
                        let cmd = v["msg"]["cmd"].as_str().unwrap_or("?").to_string();
                        (cmd, v["msg"]["data"].clone())
                    }
                    Err(_) => ("raw".to_string(), Value::Null),
                };
                // Our own multicast scan can loop back to us; ignore it.
                if cmd == "scan" && data.get("account_topic").is_some() {
                    continue;
                }
                if tx
                    .send(Incoming {
                        from: from.ip(),
                        cmd,
                        data,
                        raw,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            // Windows reports ICMP "port unreachable" from earlier sends as a
            // recv error (WSAECONNRESET); it's harmless, keep listening.
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

/// Built-in scene codes that many LAN models accept. Not universal: if a
/// scene does nothing, your model uses different codes.
pub const SCENES: &[(&str, u8)] = &[
    ("Sunrise", 0x00),
    ("Sunset", 0x01),
    ("Movie", 0x04),
    ("Dating", 0x05),
    ("Romantic", 0x07),
    ("Twinkle", 0x08),
    ("Candlelight", 0x09),
    ("Breathe", 0x0a),
    ("Snowflake", 0x0f),
    ("Energetic", 0x10),
    ("Crossing", 0x15),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ble_checksum_matches_known_keepalive() {
        // AA 01 padded, checksum AB, a well-known Govee keep-alive packet.
        let p = ble_packet(&[0xAA, 0x01]);
        assert_eq!(p[19], 0xAB);
        assert_eq!(p.len(), 20);
    }

    #[test]
    fn segment_packet_layout() {
        let mask = segment_mask(0..15);
        assert_eq!(mask, [0xFF, 0x7F]);
        let p = segment_color_packet([0xFF, 0x00, 0x00], mask);
        assert_eq!(
            &p[..14],
            &[
                0x33, 0x05, 0x15, 0x01, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0x7F
            ]
        );
        assert_eq!(&p[14..19], &[0; 5]);
        assert_eq!(p[19], xor(&p[..19]));
        // Matches the byte layout used by govee-local-api's SegmentColorMessages
        // for segment 9 (mask 00 01).
        assert_eq!(segment_mask([8]), [0x00, 0x01]);
    }

    #[test]
    fn razer_frames() {
        assert_eq!(
            razer_enable(true),
            vec![0xBB, 0x00, 0x01, 0xB1, 0x01, 0xBB ^ 0x01 ^ 0xB1 ^ 0x01]
        );
        let f = razer_frame(&[[1, 2, 3], [4, 5, 6]], true);
        assert_eq!(&f[..6], &[0xBB, 0x00, 8, 0xB0, 1, 2]);
        assert_eq!(&f[6..12], &[1, 2, 3, 4, 5, 6]);
        assert_eq!(*f.last().unwrap(), xor(&f[..f.len() - 1]));
        let big = vec![[9u8; 3]; 200];
        assert_eq!(razer_frame(&big, false)[2], (2 + 3 * RAZER_MAX_LEDS) as u8);
    }

    #[test]
    fn json_shapes() {
        let v = cmd_pt_real(&[&segment_color_packet([1, 2, 3], [1, 0])]);
        assert_eq!(v["msg"]["cmd"], "ptReal");
        let b64 = v["msg"]["data"]["command"][0].as_str().unwrap();
        assert_eq!(B64.decode(b64).unwrap().len(), 20);
        assert_eq!(cmd_color([1, 2, 3])["msg"]["data"]["color"]["g"], 2);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("33 01 01", false).unwrap(), vec![0x33, 1, 1]);
        let p = parse_hex("330101", true).unwrap();
        assert_eq!(p.len(), 20);
        assert_eq!(p[19], 0x33);
        assert!(parse_hex("3", false).is_err());
    }
}
