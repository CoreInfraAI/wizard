//! `WinINet`'s string grammar and flags, compiled and tested on every platform.
#![cfg_attr(not(any(target_os = "windows", test)), expect(dead_code))]

use crate::{ProxyState, ServiceStatus, endpoint_state};
use anyhow::{Result, ensure};

// WinINet PROXY_TYPE_* values. Kept here so policy tests need no Windows API.
pub(super) const DIRECT: u32 = 1;
pub(super) const PROXY: u32 = 2;
const AUTO_URL: u32 = 4;
const AUTO_DETECT: u32 = 8;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Config {
    pub flags: u32,
    pub server: String,
    pub bypass: String,
    pub pac: String,
}

fn entries(server: &str) -> Result<Vec<(&str, &str)>> {
    if !server.contains('=') {
        ensure!(
            !server.contains(';'),
            "unsupported system proxy server syntax"
        );
        return Ok(vec![("", server.trim())]);
    }
    let mut entries = Vec::new();
    for entry in server.split(';').filter(|s| !s.trim().is_empty()) {
        let (protocol, endpoint) = entry
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("unsupported system proxy server syntax"))?;
        let protocol = protocol.trim();
        let endpoint = endpoint.trim();
        ensure!(
            !protocol.is_empty()
                && !endpoint.is_empty()
                && !entries
                    .iter()
                    .any(|(p, _): &(&str, &str)| p.eq_ignore_ascii_case(protocol)),
            "invalid or duplicate system proxy protocol"
        );
        entries.push((protocol, endpoint));
    }
    ensure!(!entries.is_empty(), "empty system proxy protocol list");
    Ok(entries)
}

/// Whether normal enabling may replace this server string, even an inactive one:
/// only forced mode discards global or non-HTTP mappings.
fn replaceable(server: &str) -> bool {
    entries(server).is_ok_and(|entries| {
        entries.iter().all(|(protocol, endpoint)| {
            (protocol.is_empty() && (endpoint.is_empty() || *endpoint == wizard_proxy::ADDRESS))
                || protocol.eq_ignore_ascii_case("http")
                || protocol.eq_ignore_ascii_case("https")
        })
    })
}

impl Config {
    pub fn status(&self) -> Result<ServiceStatus> {
        let active = self.flags & PROXY != 0;
        let parsed = if active {
            entries(&self.server)?
        } else {
            Vec::new()
        };
        let endpoint = |protocol: &str| {
            parsed
                .iter()
                .find(|(p, _)| p.is_empty() || p.eq_ignore_ascii_case(protocol))
                .map_or("", |(_, value)| *value)
        };
        let http = endpoint_state(active && !endpoint("http").is_empty(), endpoint("http"));
        let https = endpoint_state(active && !endpoint("https").is_empty(), endpoint("https"));
        let other = parsed.iter().any(|(p, _)| {
            !p.is_empty() && !p.eq_ignore_ascii_case("http") && !p.eq_ignore_ascii_case("https")
        });
        Ok(ServiceStatus {
            id: "current-user-lan".into(),
            http,
            https,
            conflict: http == ProxyState::Foreign
                || https == ProxyState::Foreign
                || other
                || (active && self.server.trim().is_empty())
                || !replaceable(&self.server)
                || self.flags & (AUTO_URL | AUTO_DETECT) != 0
                || self.flags & !(DIRECT | PROXY | AUTO_URL | AUTO_DETECT) != 0,
        })
    }

    pub fn prepare(&self, enabled: bool, forced: bool) -> Result<Self> {
        let mut after = self.clone();
        if enabled {
            let address = wizard_proxy::ADDRESS;
            // WinINet has a single manual-proxy switch. Forced mode replaces
            // the entire manual-proxy string, including non-HTTP mappings.
            // Normal mode preserves the stricter conflict and syntax checks.
            ensure!(
                forced || replaceable(&self.server),
                "global or non-HTTP proxy mappings conflict with Wizard; use forced mode to replace manual proxy settings"
            );
            after.server = format!("http={address};https={address}");
            after.flags |= DIRECT | PROXY;
            if forced {
                after.flags &= !(AUTO_URL | AUTO_DETECT);
            }
        } else if self.flags & PROXY != 0 {
            let parsed = entries(&self.server)?;
            if parsed.len() == 1 && parsed[0].0.is_empty() {
                if parsed[0].1 == wizard_proxy::ADDRESS {
                    after.flags = (after.flags & !PROXY) | DIRECT;
                }
            } else {
                let remaining: Vec<_> = self
                    .server
                    .split(';')
                    .filter(|entry| {
                        let Some((protocol, address)) = entry.split_once('=') else {
                            return true;
                        };
                        !((protocol.trim().eq_ignore_ascii_case("http")
                            || protocol.trim().eq_ignore_ascii_case("https"))
                            && address.trim() == wizard_proxy::ADDRESS)
                    })
                    .collect();
                let server = remaining.join(";");
                if server.trim_matches(';').trim().is_empty() {
                    after.flags = (after.flags & !PROXY) | DIRECT;
                } else {
                    after.server = server;
                }
            }
        }
        Ok(after)
    }
}
