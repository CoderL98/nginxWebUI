//! Client address handling behind reverse proxies.
//!
//! `X-Forwarded-For` / `X-Forwarded-Proto` are only honoured when the TCP peer
//! is listed in `trusted_proxies` (config file, `NGINXWEBUI_TRUSTED_PROXIES`
//! or `--trusted-proxies`). The default is an empty list: the headers are
//! ignored, so a client cannot spoof its IP to dodge login/join rate limits.

use std::net::IpAddr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IpNet {
    addr: IpAddr,
    prefix: u8,
}

impl IpNet {
    pub fn parse(text: &str) -> Result<IpNet, String> {
        let text = text.trim();
        let (addr, prefix) = match text.split_once('/') {
            Some((a, p)) => (a.trim(), Some(p.trim())),
            None => (text, None),
        };
        let addr: IpAddr = addr
            .parse()
            .map_err(|_| format!("invalid trusted proxy address `{text}`"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("invalid prefix in `{text}`"))?,
            None => max,
        };
        Ok(IpNet { addr, prefix })
    }

    pub fn contains(&self, ip: &IpAddr) -> bool {
        let ip = canonical(*ip);
        match (canonical(self.addr), ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let bits = u32::from(net) ^ u32::from(ip);
                self.prefix == 0 || bits >> (32 - self.prefix.min(32) as u32) == 0
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let bits = u128::from(net) ^ u128::from(ip);
                self.prefix == 0 || bits >> (128 - self.prefix.min(128) as u32) == 0
            }
            _ => false,
        }
    }
}

fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        other => other,
    }
}

/// Parse a comma/space separated list such as `127.0.0.1, 172.16.0.0/12`.
pub fn parse_list(text: &str) -> Result<Vec<IpNet>, String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.trim().is_empty())
        .map(IpNet::parse)
        .collect()
}

pub fn is_trusted(peer: Option<IpAddr>, trusted: &[IpNet]) -> bool {
    match peer {
        Some(ip) => trusted.iter().any(|net| net.contains(&ip)),
        None => false,
    }
}

/// The client address: the TCP peer, or — when the peer is a trusted proxy —
/// the right-most `X-Forwarded-For` entry that is not itself a trusted proxy.
pub fn client_ip(peer: Option<IpAddr>, forwarded_for: &str, trusted: &[IpNet]) -> String {
    let peer_text = peer
        .map(|ip| canonical(ip).to_string())
        .unwrap_or_else(|| "unknown".into());
    if forwarded_for.trim().is_empty() || !is_trusted(peer, trusted) {
        return peer_text;
    }
    for hop in forwarded_for.rsplit(',') {
        let hop = hop.trim();
        let Ok(ip) = hop.parse::<IpAddr>() else {
            return peer_text;
        };
        if !trusted.iter().any(|net| net.contains(&ip)) {
            return canonical(ip).to_string();
        }
    }
    peer_text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn cidr_matching() {
        let net = IpNet::parse("172.16.0.0/12").unwrap();
        assert!(net.contains(&"172.17.0.1".parse().unwrap()));
        assert!(!net.contains(&"10.0.0.1".parse().unwrap()));
        let single = IpNet::parse("127.0.0.1").unwrap();
        assert!(single.contains(&"::ffff:127.0.0.1".parse().unwrap()));
        assert!(IpNet::parse("1.2.3.4/40").is_err());
        assert!(IpNet::parse("nope").is_err());
        assert!(IpNet::parse("::1")
            .unwrap()
            .contains(&"::1".parse().unwrap()));
    }

    #[test]
    fn forwarded_for_is_ignored_without_trusted_proxy() {
        assert_eq!(client_ip(ip("203.0.113.9"), "1.1.1.1", &[]), "203.0.113.9");
    }

    #[test]
    fn forwarded_for_uses_rightmost_untrusted_hop() {
        let trusted = parse_list("127.0.0.1, 10.0.0.0/8").unwrap();
        assert_eq!(
            client_ip(ip("127.0.0.1"), "6.6.6.6, 198.51.100.7, 10.1.1.1", &trusted),
            "198.51.100.7"
        );
        // spoofed garbage falls back to the peer
        assert_eq!(client_ip(ip("127.0.0.1"), "evil", &trusted), "127.0.0.1");
        // untrusted peer: header ignored
        assert_eq!(
            client_ip(ip("192.0.2.1"), "198.51.100.7", &trusted),
            "192.0.2.1"
        );
    }
}
