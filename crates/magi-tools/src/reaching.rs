//! Where a `reach` grant may not go, whatever a name resolves to.
//!
//! A permitted hostname is only as safe as its DNS: whoever controls the name controls the address,
//! and an address inside this machine — loopback, the cloud's metadata endpoint, a link-local
//! neighbour — is not what "reach the internet" was granted for. So a network action is judged by
//! the address its host resolves to, not by the name, and the addresses below are refused however
//! they were reached. Ported from Anthropic's sandbox-runtime, which calls it the resolved-address
//! guard; the network is otherwise all-or-nothing here until Landlock narrows it per host.

use std::net::{IpAddr, Ipv6Addr};

/// Why an address is refused, or `None` when it may be reached.
#[must_use]
pub fn denied(ip: IpAddr) -> Option<&'static str> {
    match ip {
        IpAddr::V4(v4) => {
            if v4.is_loopback() {
                Some("loopback")
            } else if v4.is_unspecified() {
                Some("this host")
            } else if v4.is_link_local() {
                Some("link-local")
            } else if v4.is_multicast() {
                Some("multicast")
            } else if v4.is_broadcast() {
                Some("broadcast")
            } else if matches!(
                v4.octets(),
                [100, 100, 100, 200] | [168, 63, 129, 16] | [192, 0, 0, 192]
            ) {
                // Cloud instance-metadata endpoints that live outside link-local.
                Some("cloud metadata")
            } else {
                None
            }
        }
        IpAddr::V6(v6) => {
            if v6.is_loopback() {
                Some("loopback")
            } else if v6.is_unspecified() {
                Some("this host")
            } else if v6.is_multicast() {
                Some("multicast")
            } else if (v6.segments()[0] & 0xffc0) == 0xfe80 {
                Some("link-local")
            } else if let Some(v4) = mapped(v6) {
                // An IPv6 form carrying an IPv4 address is judged by the address it embeds.
                denied(IpAddr::V4(v4))
            } else {
                None
            }
        }
    }
}

/// The IPv4 address an IPv4-mapped IPv6 address carries (`::ffff:a.b.c.d`), if it is one.
fn mapped(v6: Ipv6Addr) -> Option<std::net::Ipv4Addr> {
    let o = v6.octets();
    (o[..10].iter().all(|b| *b == 0) && o[10] == 0xff && o[11] == 0xff)
        .then(|| std::net::Ipv4Addr::new(o[12], o[13], o[14], o[15]))
}

/// Refuse a host that resolves only into addresses [`denied`] rejects, so a name pointed at the
/// machine's own insides is caught. A host that will not resolve is left to the connection to fail;
/// an IP literal is judged directly. `host` may carry a `:port`, which is dropped for the check.
///
/// # Errors
/// When every address the host resolves to is one that may not be reached.
pub fn guard(host: &str) -> Result<(), String> {
    let name = host.rsplit_once(':').map_or(host, |(head, _)| head);
    if let Ok(ip) = name.parse::<IpAddr>() {
        return match denied(ip) {
            Some(why) => Err(format!(
                "{host} is a {why} address, which a reach grant does not cover"
            )),
            None => Ok(()),
        };
    }
    // Resolve to judge the addresses; a name that does not resolve is not refused here.
    let Ok(resolved) = std::net::ToSocketAddrs::to_socket_addrs(&(name, 0)) else {
        return Ok(());
    };
    let addrs: Vec<IpAddr> = resolved.map(|s| s.ip()).collect();
    if !addrs.is_empty() && addrs.iter().all(|ip| denied(*ip).is_some()) {
        let why = addrs
            .first()
            .and_then(|ip| denied(*ip))
            .unwrap_or("blocked");
        return Err(format!(
            "{host} resolves only to a {why} address, which a reach grant does not cover"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    #[test]
    fn the_machines_own_insides_are_denied() {
        assert_eq!(denied(v4(127, 0, 0, 1)), Some("loopback"));
        assert_eq!(denied(v4(0, 0, 0, 0)), Some("this host"));
        assert_eq!(denied(v4(169, 254, 1, 2)), Some("link-local"));
        assert_eq!(
            denied(v4(169, 254, 169, 254)),
            Some("link-local"),
            "the metadata IP"
        );
        assert_eq!(denied(v4(100, 100, 100, 200)), Some("cloud metadata"));
        assert_eq!(denied("::1".parse().expect("an address")), Some("loopback"));
        assert_eq!(
            denied("fe80::1".parse().expect("an address")),
            Some("link-local")
        );
        assert_eq!(
            denied("::ffff:127.0.0.1".parse().expect("an address")),
            Some("loopback"),
            "mapped v4"
        );
    }

    #[test]
    fn an_ordinary_address_is_allowed() {
        assert_eq!(denied(v4(140, 82, 121, 3)), None, "a public host");
        assert_eq!(denied("2606:4700::1".parse().expect("an address")), None);
    }

    #[test]
    fn guard_refuses_a_loopback_literal_and_allows_a_public_one() {
        assert!(guard("127.0.0.1:8080").is_err());
        assert!(
            guard("169.254.169.254").is_err(),
            "the metadata endpoint by literal"
        );
        assert!(guard("140.82.121.3:443").is_ok());
    }

    #[test]
    fn guard_lets_an_unresolvable_name_through_to_fail_on_connect() {
        assert!(guard("no-such-host.invalid").is_ok());
    }
}
