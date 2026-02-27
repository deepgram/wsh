//! IP access control for SSRF mitigation in federation backend connections.
//!
//! Provides configurable blocklist/allowlist based on CIDR ranges. Evaluation
//! order: blocklist denies first → allowlist permits → default policy.
//!
//! If an allowlist is configured, only matching IPs are allowed (implicit deny).
//! If no allowlist is configured, all non-blocked IPs are allowed (implicit allow).
//!
//! There is **no hardcoded blocklist**. The user owns the threat model entirely.
//! Loopback addresses are legitimate for local federation testing.

use ipnet::IpNet;
use std::net::IpAddr;

use crate::config::IpAccessConfig;

/// Parsed IP access control rules.
#[derive(Debug)]
pub struct IpAccessControl {
    blocklist: Vec<IpNet>,
    allowlist: Vec<IpNet>,
}

impl IpAccessControl {
    /// Build from the config representation, parsing CIDR strings.
    ///
    /// Invalid CIDR entries are logged and skipped rather than causing a fatal error.
    pub fn from_config(config: &IpAccessConfig) -> Self {
        let blocklist = config
            .blocklist
            .iter()
            .filter_map(|s| {
                s.parse::<IpNet>()
                    .map_err(|e| tracing::warn!(cidr = %s, ?e, "invalid blocklist CIDR, skipping"))
                    .ok()
            })
            .collect();
        let allowlist = config
            .allowlist
            .iter()
            .filter_map(|s| {
                s.parse::<IpNet>()
                    .map_err(|e| tracing::warn!(cidr = %s, ?e, "invalid allowlist CIDR, skipping"))
                    .ok()
            })
            .collect();
        Self {
            blocklist,
            allowlist,
        }
    }

    /// Check whether the given IP address is allowed.
    ///
    /// Returns `Ok(())` if allowed, or `Err(reason)` if denied.
    pub fn check(&self, ip: IpAddr) -> Result<(), String> {
        // 1. Blocklist denies first
        for net in &self.blocklist {
            if net.contains(&ip) {
                return Err(format!("IP {} is in blocklist ({})", ip, net));
            }
        }

        // 2. If allowlist is configured, only listed CIDRs pass
        if !self.allowlist.is_empty() {
            for net in &self.allowlist {
                if net.contains(&ip) {
                    return Ok(());
                }
            }
            return Err(format!(
                "IP {} is not in allowlist",
                ip
            ));
        }

        // 3. No allowlist → allow by default
        Ok(())
    }

    /// Returns true if both blocklist and allowlist are empty (no access control configured).
    pub fn is_unconfigured(&self) -> bool {
        self.blocklist.is_empty() && self.allowlist.is_empty()
    }
}

/// Resolve a backend address URL and check all resolved IPs against the access control.
///
/// Extracts the host:port from the URL, resolves it via DNS (or parses it as an IP
/// literal), and checks every resolved address. Returns `Ok(())` if all pass,
/// `Err(reason)` if any resolved IP is denied.
pub async fn check_backend_url(
    ctrl: &IpAccessControl,
    address: &str,
) -> Result<(), String> {
    // Extract authority (host:port) from the URL. The address must already be
    // validated by validate_backend_address() before calling this.
    let authority = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"))
        .ok_or_else(|| format!("address missing scheme: {}", address))?;

    // Extract host:port (strip path if present)
    let host_port = authority.split('/').next().unwrap_or(authority);

    // If no port, add default based on scheme
    let host_port = if host_port.contains(':') || host_port.starts_with('[') {
        host_port.to_string()
    } else if address.starts_with("https://") {
        format!("{}:443", host_port)
    } else {
        format!("{}:80", host_port)
    };

    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host(&host_port)
        .await
        .map_err(|e| format!("DNS resolution failed for {}: {}", host_port, e))?
        .collect();

    if addrs.is_empty() {
        return Err(format!("DNS resolution returned no addresses for {}", host_port));
    }

    for addr in &addrs {
        ctrl.check(addr.ip())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(blocklist: &[&str], allowlist: &[&str]) -> IpAccessConfig {
        IpAccessConfig {
            blocklist: blocklist.iter().map(|s| s.to_string()).collect(),
            allowlist: allowlist.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn empty_config_allows_everything() {
        let ctrl = IpAccessControl::from_config(&make_config(&[], &[]));
        assert!(ctrl.is_unconfigured());
        assert!(ctrl.check("10.0.1.1".parse().unwrap()).is_ok());
        assert!(ctrl.check("127.0.0.1".parse().unwrap()).is_ok());
        assert!(ctrl.check("::1".parse().unwrap()).is_ok());
    }

    #[test]
    fn blocklist_denies_matching_ip() {
        let ctrl = IpAccessControl::from_config(&make_config(
            &["169.254.0.0/16"],
            &[],
        ));
        assert!(!ctrl.is_unconfigured());
        assert!(ctrl.check("169.254.1.1".parse().unwrap()).is_err());
        assert!(ctrl.check("10.0.1.1".parse().unwrap()).is_ok());
    }

    #[test]
    fn blocklist_ipv6() {
        let ctrl = IpAccessControl::from_config(&make_config(
            &["::1/128"],
            &[],
        ));
        assert!(ctrl.check("::1".parse().unwrap()).is_err());
        assert!(ctrl.check("::2".parse().unwrap()).is_ok());
    }

    #[test]
    fn allowlist_restricts_to_listed_cidrs() {
        let ctrl = IpAccessControl::from_config(&make_config(
            &[],
            &["10.0.0.0/8", "192.168.0.0/16"],
        ));
        assert!(ctrl.check("10.0.1.1".parse().unwrap()).is_ok());
        assert!(ctrl.check("192.168.1.1".parse().unwrap()).is_ok());
        assert!(ctrl.check("172.16.0.1".parse().unwrap()).is_err());
        assert!(ctrl.check("8.8.8.8".parse().unwrap()).is_err());
    }

    #[test]
    fn blocklist_takes_precedence_over_allowlist() {
        let ctrl = IpAccessControl::from_config(&make_config(
            &["10.0.1.0/24"],
            &["10.0.0.0/8"],
        ));
        // 10.0.1.1 is in both blocklist and allowlist — blocklist wins
        assert!(ctrl.check("10.0.1.1".parse().unwrap()).is_err());
        // 10.0.2.1 is in allowlist but not blocklist — allowed
        assert!(ctrl.check("10.0.2.1".parse().unwrap()).is_ok());
        // 172.16.0.1 is in neither — denied by allowlist
        assert!(ctrl.check("172.16.0.1".parse().unwrap()).is_err());
    }

    #[test]
    fn single_host_cidr() {
        let ctrl = IpAccessControl::from_config(&make_config(
            &["192.168.1.100/32"],
            &[],
        ));
        assert!(ctrl.check("192.168.1.100".parse().unwrap()).is_err());
        assert!(ctrl.check("192.168.1.101".parse().unwrap()).is_ok());
    }

    #[test]
    fn invalid_cidr_skipped() {
        let ctrl = IpAccessControl::from_config(&make_config(
            &["not-a-cidr", "10.0.0.0/8"],
            &[],
        ));
        // Invalid entry is skipped, valid entry still works
        assert!(ctrl.check("10.0.1.1".parse().unwrap()).is_err());
        assert!(ctrl.check("172.16.0.1".parse().unwrap()).is_ok());
    }

    #[test]
    fn loopback_allowed_by_default() {
        // No hardcoded blocklist — loopback is legitimate for local federation testing
        let ctrl = IpAccessControl::from_config(&make_config(&[], &[]));
        assert!(ctrl.check("127.0.0.1".parse().unwrap()).is_ok());
        assert!(ctrl.check("::1".parse().unwrap()).is_ok());
    }

    #[test]
    fn loopback_blocked_when_configured() {
        let ctrl = IpAccessControl::from_config(&make_config(
            &["127.0.0.0/8", "::1/128"],
            &[],
        ));
        assert!(ctrl.check("127.0.0.1".parse().unwrap()).is_err());
        assert!(ctrl.check("::1".parse().unwrap()).is_err());
    }
}
