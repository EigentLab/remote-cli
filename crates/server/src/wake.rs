//! Wake Coordinator: picks Wake Relays (Online Agents on the target's subnet) from the LANs they report.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use rcli_protocol::agent::NetworkInterface;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What an Address Book keeps to wake a Device: its MAC and the subnet it was on, as `"aa:bb:cc:dd:ee:ff"` and
/// `"192.168.1.0/24"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Address {
    pub mac: String,
    pub subnet: String,
}

/// The first interface with a MAC and a LAN address, as an Address; None when there is none.
pub fn address(interfaces: &[NetworkInterface]) -> Option<Address> {
    interfaces.iter().find_map(|i| {
        let mac = parse_mac(&i.mac)?;
        let (ip, prefix) = i.ips.iter().find_map(|ip| net(ip))?;
        Some(Address { mac: format_mac(mac), subnet: format!("{}/{prefix}", masked(ip, prefix)) })
    })
}

/// The target's MAC and the relays (in `relays` order) with an address in its subnet, or why there are none.
pub fn routes(
    target_name: &str,
    target: &Address,
    relays: &[(Uuid, &[NetworkInterface])],
) -> Result<([u8; 6], Vec<Uuid>), String> {
    let (mac, lan, prefix) = parse(target)?;
    let found: Vec<Uuid> = relays
        .iter()
        .filter(|(_, ifaces)| {
            ifaces.iter().flat_map(|i| &i.ips).filter_map(|ip| net(ip)).any(|(r, _)| masked(r, prefix) == lan)
        })
        .map(|(id, _)| *id)
        .collect();
    if found.is_empty() {
        return Err(format!(
            "no Online Agent is in the LAN of Device {target_name:?} ({lan}/{prefix}); keep an always-on Device with an Agent there to act as Wake Relay"
        ));
    }
    Ok((mac, found))
}

/// The Address's MAC, network address and prefix, or what is wrong with it.
pub fn parse(a: &Address) -> Result<([u8; 6], IpAddr, u8), String> {
    let mac = parse_mac(&a.mac).ok_or_else(|| format!("{:?} is not a MAC address", a.mac))?;
    let (ip, prefix) = Some(&a.subnet)
        .filter(|s| s.contains('/'))
        .and_then(|s| net(s))
        .ok_or_else(|| format!("{:?} is not a LAN subnet like 192.168.1.0/24", a.subnet))?;
    Ok((mac, masked(ip, prefix), prefix))
}

/// `"addr/prefix"` -> (addr, prefix); None for unparsable, loopback and link-local (they match across LANs).
fn net(s: &str) -> Option<(IpAddr, u8)> {
    let (ip, prefix) = match s.split_once('/') {
        Some((ip, p)) => (ip.parse::<IpAddr>().ok()?, p.parse().ok()?),
        // shortcut: no prefix reported = assume /24 (v4) or /64 (v6); fine for typical LANs.
        None => {
            let ip: IpAddr = s.parse().ok()?;
            (ip, if ip.is_ipv4() { 24 } else { 64 })
        }
    };
    let local = match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || prefix > 32,
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.segments()[0] & 0xffc0 == 0xfe80 || v6.is_unspecified() || prefix > 128
        }
    };
    (!local).then_some((ip, prefix))
}

/// The network address of `ip` under `prefix` (an IPv4 address never equals an IPv6 one).
fn masked(ip: IpAddr, prefix: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            IpAddr::V4(Ipv4Addr::from(u32::from(v4) & u32::MAX.checked_shl(32 - prefix.min(32) as u32).unwrap_or(0)))
        }
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(
            u128::from(v6) & u128::MAX.checked_shl(128 - prefix.min(128) as u32).unwrap_or(0),
        )),
    }
}

/// "aa:bb:cc:dd:ee:ff" (or with '-'), case-insensitive; None if malformed or all zero (loopback).
fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let bytes: Vec<u8> = s.split([':', '-']).map(|b| u8::from_str_radix(b, 16).ok()).collect::<Option<_>>()?;
    let mac: [u8; 6] = bytes.try_into().ok()?;
    (mac != [0; 6]).then_some(mac)
}

pub fn format_mac(mac: [u8; 6]) -> String {
    mac.map(|b| format!("{b:02x}")).join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ifaces(list: &[(&str, &[&str])]) -> Vec<NetworkInterface> {
        list.iter()
            .map(|(mac, ips)| NetworkInterface {
                name: "eth".into(),
                mac: mac.to_string(),
                ips: ips.iter().map(|s| s.to_string()).collect(),
            })
            .collect()
    }

    #[test]
    fn the_address_is_the_first_interface_with_a_mac_on_a_lan() {
        let target =
            ifaces(&[("00:00:00:00:00:00", &["127.0.0.1/8"]), ("AA-BB-CC-00-00-01", &["fe80::1/64", "10.1.2.3/16"])]);
        let want = Address { mac: "aa:bb:cc:00:00:01".into(), subnet: "10.1.0.0/16".into() };
        assert_eq!(address(&target), Some(want));
        assert_eq!(address(&ifaces(&[("aa:bb:cc:00:00:01", &["fe80::1/64"])])), None);
    }

    #[test]
    fn picks_relays_on_the_subnet_v4_and_v6() {
        let target = Address { mac: "aa:bb:cc:00:00:01".into(), subnet: "10.1.0.0/16".into() };
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let loopback = ifaces(&[("00:00:00:00:00:00", &["127.0.0.1/8", "fe80::2/64"])]);
        let same = ifaces(&[("02:00:00:00:00:02", &["10.1.200.9/16"])]);
        let other = ifaces(&[("02:00:00:00:00:03", &["10.2.2.3/16"])]);
        let relays = [(a, loopback.as_slice()), (b, same.as_slice()), (c, other.as_slice())];
        assert_eq!(routes("t", &target, &relays).unwrap(), ([0xaa, 0xbb, 0xcc, 0, 0, 1], vec![b]));

        let v6 = Address { mac: "aa:bb:cc:00:00:01".into(), subnet: "2001:db8:1::/64".into() };
        let on_v6 = ifaces(&[("02:00:00:00:00:02", &["2001:db8:1::9/64"])]);
        assert_eq!(routes("t", &v6, &[(a, on_v6.as_slice())]).unwrap().1, vec![a]);

        let far = routes("t", &Address { subnet: "192.168.1.0/24".into(), ..target }, &relays).unwrap_err();
        assert!(far.contains("192.168.1.0/24"), "{far}");
    }

    #[test]
    fn formats_macs_lowercase_with_colons() {
        assert_eq!(format_mac([0xaa, 0xbb, 0xcc, 0, 0, 1]), "aa:bb:cc:00:00:01");
    }
}
