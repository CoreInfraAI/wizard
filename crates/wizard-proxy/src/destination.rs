//! Destination parsing and matching ported from thinker, revision
//! f200242dc8030b465da6ed389310c001923904b2:

use core::{fmt, net::SocketAddr, str::FromStr};
use std::sync::LazyLock;

const ALLOWED_DESTINATIONS: &[&str] = &[
    "auth.openai.com:443",
    "chatgpt.com:443",
    "openai.com:443",
    "releases.openai.com:443",
    "api.openai.com:443",
    "developers.openai.com:443",
    "files.openai.com:443",
    "platform.openai.com:443",
    "persistent.oaistatic.com:443",
    "claude.com:443",
    "claude.ai:443",
    "assets-proxy.anthropic.com:443",
    "bridge.claudeusercontent.com:443",
    "api.anthropic.com:443",
    "platform.claude.com:443",
    "code.claude.com:443",
    "downloads.claude.ai:443",
    "releases.claude.com:443",
    "mcp-proxy.anthropic.com:443",
    "*.cursor.sh:443",
    "*.cursor-cdn.com:443",
    "*.cursorapi.com:443",
    "*.cursorvm.com:443",
    "*.*.cursorvm.com:443",
    "api.origin.cursor.com:443",
    "cursor.com:443",
    "prod.authentication.cursor.sh:443",
    "downloads.cursor.com:443",
    "cursor-cdn.com:443",
    "anysphere-binaries.s3.us-east-1.amazonaws.com:443",
    "api.x.ai:443",
    "accounts.x.ai:443",
    "accounts.spacex.ai:443",
    "auth.x.ai:443",
    "auth.grok.com:443",
    "auth.grokusercontent.com:443",
    "auth.grokipedia.com:443",
];

static PATTERNS: LazyLock<Vec<ConnectDestinationPattern>> = LazyLock::new(|| {
    ALLOWED_DESTINATIONS
        .iter()
        .map(|value| value.parse().expect("valid built-in destination pattern"))
        .collect()
});

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// The domain and port match the list; only CONNECT may use the upstream.
    Proxy,
    /// No domain pattern matches, or the destination is an explicit IP address.
    Direct,
    /// A domain pattern matches, but none permits this port.
    Block,
}

/// Classify an explicit host:port, keeping malformed input distinct from policy denial.
pub(crate) fn route(value: &str) -> Result<Route, InvalidDestination> {
    if let Ok(address) = value.parse::<SocketAddr>() {
        return if address.port() == 0 {
            Err(InvalidDestination)
        } else {
            Ok(Route::Direct)
        };
    }

    let destination = value.parse::<ConnectDestination>()?;
    Ok(route_destination(destination, &PATTERNS))
}

fn route_destination(
    mut destination: ConnectDestination,
    patterns: &[ConnectDestinationPattern],
) -> Route {
    // Check all complete matches first: overlapping patterns may permit different ports.
    if patterns.iter().any(|pattern| pattern.matches(&destination)) {
        return Route::Proxy;
    }

    // Wizard-only policy: check the same host at each pattern's permitted port.
    // Reuse thinker's unchanged matcher instead of duplicating wildcard semantics.
    for pattern in patterns {
        destination.port = pattern.port;
        if pattern.matches(&destination) {
            return Route::Block;
        }
    }
    Route::Direct
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InvalidDestination;

impl fmt::Display for InvalidDestination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid proxy destination")
    }
}

impl core::error::Error for InvalidDestination {}

/// An exact lowercase ASCII DNS name with an explicit nonzero TCP port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnectDestination {
    host: String,
    port: u16,
}

impl ConnectDestination {
    fn host(&self) -> &str {
        &self.host
    }

    const fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for ConnectDestination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.host, self.port)
    }
}

impl FromStr for ConnectDestination {
    type Err = InvalidDestination;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value != value.trim() {
            return Err(InvalidDestination);
        }
        let (host, port) = value.rsplit_once(':').ok_or(InvalidDestination)?;
        if host.contains(':') {
            return Err(InvalidDestination);
        }
        validate_dns_host(host)?;
        if !matches!(url::Host::parse(host), Ok(url::Host::Domain(_)))
            || port.is_empty()
            || !port.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(InvalidDestination);
        }
        let port = port.parse::<u16>().map_err(|_| InvalidDestination)?;
        if port == 0 {
            return Err(InvalidDestination);
        }

        Ok(Self {
            host: host.to_ascii_lowercase(),
            port,
        })
    }
}

fn validate_dns_host(host: &str) -> Result<(), InvalidDestination> {
    if host.is_empty() || host.len() > 253 || !host.is_ascii() || host.ends_with('.') {
        return Err(InvalidDestination);
    }

    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            || label.starts_with('-')
            || label.ends_with('-')
        {
            return Err(InvalidDestination);
        }
    }
    Ok(())
}

/// Exact names or consecutive leading `*` labels followed by at least two
/// fixed DNS labels. Each star matches exactly one label, not an arbitrary glob.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ConnectDestinationPattern {
    host: String,
    port: u16,
}

impl ConnectDestinationPattern {
    fn host(&self) -> &str {
        &self.host
    }

    fn matches(&self, destination: &ConnectDestination) -> bool {
        if self.port != destination.port() {
            return false;
        }
        let mut labels = destination.host().split('.');
        for pattern in self.host().split('.') {
            let Some(label) = labels.next() else {
                return false;
            };
            if pattern != "*" && pattern != label {
                return false;
            }
        }
        labels.next().is_none()
    }
}

impl From<ConnectDestination> for ConnectDestinationPattern {
    fn from(destination: ConnectDestination) -> Self {
        Self {
            host: destination.host,
            port: destination.port,
        }
    }
}

impl fmt::Display for ConnectDestinationPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.host, self.port)
    }
}

impl FromStr for ConnectDestinationPattern {
    type Err = InvalidDestination;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if !value.contains('*') {
            return value.parse::<ConnectDestination>().map(Self::from);
        }
        if value != value.trim() {
            return Err(InvalidDestination);
        }
        let (host, port) = value.rsplit_once(':').ok_or(InvalidDestination)?;
        if host.len() > 253 {
            return Err(InvalidDestination);
        }
        let mut suffix = host;
        while let Some(rest) = suffix.strip_prefix("*.") {
            suffix = rest;
        }
        if suffix == host || suffix.contains('*') || suffix.split('.').count() < 2 {
            return Err(InvalidDestination);
        }
        // Reuse the concrete parser for suffix, IP-literal and port validation.
        let destination = format!("{suffix}:{port}").parse::<ConnectDestination>()?;
        Ok(Self {
            host: host.to_ascii_lowercase(),
            port: destination.port,
        })
    }
}

#[cfg(test)]
#[path = "destination_test.rs"]
mod tests;
