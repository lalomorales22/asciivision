//! Short human-friendly room codes.
//!
//! A room code is a Crockford-style base32 encoding of an IPv4 address plus
//! port (48 bits -> 10 symbols, shown as "XXXXX-XXXXX"). The alphabet drops
//! the confusable glyphs 0/O and 1/I entirely, so a code can be read out
//! loud across a room without ambiguity. Decoding is case-insensitive and
//! ignores dashes and whitespace.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

/// 32 symbols, no 0/O/1/I.
const ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";

/// Encode an IPv4 socket address as a room code like "K7QM3-XZ2AB".
pub fn encode(addr: SocketAddrV4) -> String {
    let ip = u32::from(*addr.ip()) as u64;
    let value: u64 = (ip << 16) | addr.port() as u64; // 48 bits
    let mut out = String::with_capacity(11);
    for symbol in (0..10).rev() {
        let index = ((value >> (symbol * 5)) & 0x1f) as usize;
        out.push(ALPHABET[index] as char);
        if symbol == 5 {
            out.push('-');
        }
    }
    out
}

/// Decode a room code back into an IPv4 socket address.
/// Case-insensitive; dashes and whitespace are ignored; anything else
/// (wrong length, symbols outside the alphabet) returns None.
pub fn decode(code: &str) -> Option<SocketAddrV4> {
    let mut value: u64 = 0;
    let mut symbols = 0usize;
    for ch in code.chars() {
        if ch == '-' || ch.is_whitespace() {
            continue;
        }
        let upper = ch.to_ascii_uppercase();
        let index = ALPHABET.iter().position(|&a| a as char == upper)?;
        value = (value << 5) | index as u64;
        symbols += 1;
        if symbols > 10 {
            return None;
        }
    }
    if symbols != 10 || value >> 48 != 0 {
        return None;
    }
    let ip = Ipv4Addr::from((value >> 16) as u32);
    let port = (value & 0xffff) as u16;
    Some(SocketAddrV4::new(ip, port))
}

/// Best-effort LAN IP detection: "connect" a UDP socket toward a public
/// address and read the local address the OS picked. No packets are sent
/// (UDP connect only sets the default destination). Falls back to loopback.
pub fn lan_ip() -> Ipv4Addr {
    fn detect() -> Option<Ipv4Addr> {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect("8.8.8.8:80").ok()?;
        match socket.local_addr().ok()? {
            std::net::SocketAddr::V4(addr) => Some(*addr.ip()),
            std::net::SocketAddr::V6(_) => None,
        }
    }
    detect().unwrap_or(Ipv4Addr::LOCALHOST)
}

/// Parse "ws://1.2.3.4:9999" (or "wss://", or a bare "1.2.3.4:9999") into a
/// socket address. Hostnames are not resolved -- IPv4 literals only.
pub fn parse_ws_url(url: &str) -> Option<SocketAddrV4> {
    let rest = url
        .trim()
        .strip_prefix("ws://")
        .or_else(|| url.trim().strip_prefix("wss://"))
        .unwrap_or_else(|| url.trim());
    let rest = rest.split('/').next().unwrap_or(rest);
    rest.parse::<SocketAddrV4>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_typical_lan_addresses() {
        let cases = [
            SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 5), 9999),
            SocketAddrV4::new(Ipv4Addr::new(10, 0, 0, 42), 1),
            SocketAddrV4::new(Ipv4Addr::new(172, 16, 254, 254), 65535),
            SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 0),
            SocketAddrV4::new(Ipv4Addr::new(255, 255, 255, 255), 65535),
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, 9999),
        ];
        for addr in cases {
            let code = encode(addr);
            assert_eq!(decode(&code), Some(addr), "roundtrip failed for {}", addr);
        }
    }

    #[test]
    fn code_shape_is_grouped_and_clean() {
        let code = encode(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 5), 9999));
        assert_eq!(code.len(), 11);
        assert_eq!(code.chars().nth(5), Some('-'));
        for ch in code.chars().filter(|&c| c != '-') {
            assert!(
                !"0O1I".contains(ch),
                "confusable char {} leaked into code {}",
                ch,
                code
            );
            assert!(ALPHABET.contains(&(ch as u8)), "{} not in alphabet", ch);
        }
    }

    #[test]
    fn decode_is_case_insensitive() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(192, 168, 4, 20), 8080);
        let code = encode(addr);
        assert_eq!(decode(&code.to_lowercase()), Some(addr));
        // mixed case
        let mixed: String = code
            .chars()
            .enumerate()
            .map(|(i, c)| if i % 2 == 0 { c.to_ascii_lowercase() } else { c })
            .collect();
        assert_eq!(decode(&mixed), Some(addr));
    }

    #[test]
    fn decode_ignores_dashes_and_whitespace() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(10, 1, 2, 3), 12345);
        let code = encode(addr);
        let bare: String = code.chars().filter(|&c| c != '-').collect();
        assert_eq!(decode(&bare), Some(addr));
        // extra separators anywhere
        let spread: String = bare
            .chars()
            .flat_map(|c| [c, '-'])
            .collect();
        assert_eq!(decode(&spread), Some(addr));
        assert_eq!(decode(&format!("  {}  ", code)), Some(addr));
        let spaced = bare
            .chars()
            .map(|c| format!("{} ", c))
            .collect::<String>();
        assert_eq!(decode(&spaced), Some(addr));
    }

    #[test]
    fn garbage_inputs_are_rejected() {
        assert_eq!(decode(""), None);
        assert_eq!(decode("-"), None);
        assert_eq!(decode("hello world"), None);
        assert_eq!(decode("K7QM3"), None); // too short
        assert_eq!(decode("222222222222"), None); // too long (12 symbols)
        assert_eq!(decode("K7QM3-XZ2A!"), None); // bad symbol
        assert_eq!(decode("K7QM3-XZ200"), None); // '0' not in alphabet
        assert_eq!(decode("K7QM3-XZ2OI"), None); // 'O'/'I' not in alphabet
        assert_eq!(decode("ZZZZZ-ZZZZZ"), None); // overflows 48 bits
        assert_eq!(decode("ws://1.2.3.4:5"), None);
    }

    #[test]
    fn parse_ws_url_variants() {
        let addr = SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 9), 9999);
        assert_eq!(parse_ws_url("ws://192.168.1.9:9999"), Some(addr));
        assert_eq!(parse_ws_url("wss://192.168.1.9:9999"), Some(addr));
        assert_eq!(parse_ws_url("192.168.1.9:9999"), Some(addr));
        assert_eq!(parse_ws_url("ws://192.168.1.9:9999/room"), Some(addr));
        assert_eq!(parse_ws_url(" ws://192.168.1.9:9999 "), Some(addr));
        assert_eq!(parse_ws_url("ws://example.com:9999"), None); // hostnames unsupported
        assert_eq!(parse_ws_url("nonsense"), None);
    }

    #[test]
    fn lan_ip_returns_something_usable() {
        let ip = lan_ip();
        // whatever the environment, it must be a valid IPv4 we can encode
        let addr = SocketAddrV4::new(ip, 9999);
        assert_eq!(decode(&encode(addr)), Some(addr));
    }
}
