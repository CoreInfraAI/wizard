//! System HTTP/HTTPS proxy settings, independent of the listener's lifecycle.
//!
//! Blocking API: callers must use a worker thread. macOS mutations may request
//! authorization. No API starts the proxy, changes its token, or checks its health.
//! Settings are identified by endpoint, not proof of ownership: another program
//! using exactly the same endpoint is indistinguishable from Wizard.

use anyhow::{Context as _, Result, ensure};
use std::sync::Mutex;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;
mod windows_config;

#[cfg(target_os = "macos")]
use macos::Native;
#[cfg(target_os = "windows")]
use windows::Native;

#[cfg(test)]
mod tests;

static LOCK: Mutex<()> = Mutex::new(());

/// Authorization was not granted before any OS settings were staged or written.
/// Callers may offer an authorization retry without a partial-write warning.
#[derive(Debug)]
pub struct AuthorizationRequired {
    pub code: i32,
}

impl core::fmt::Display for AuthorizationRequired {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "system proxy authorization was not granted (native code {}); OS settings were not changed",
            self.code,
        )
    }
}

impl core::error::Error for AuthorizationRequired {}

/// Configuration state, not listener availability or application traffic routing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxyState {
    Disabled,
    Wizard,
    Foreign,
}

/// No endpoints, PAC URLs, credentials or complete OS settings are exposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceStatus {
    pub id: String,
    pub http: ProxyState,
    pub https: ProxyState,
    pub conflict: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemProxyStatus {
    pub services: Vec<ServiceStatus>,
}

/// Read supported services. Does not request authorization.
pub fn status() -> Result<SystemProxyStatus> {
    let _guard = LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("system proxy lock poisoned"))?;
    native_status()
}

/// Set HTTP and HTTPS to the Wizard endpoint. With `forced`, replace foreign
/// HTTP/HTTPS settings and disable PAC/auto-discovery. On macOS, forced mode also
/// disables SOCKS, FTP, RTSP and Gopher proxies. On Windows, forced mode
/// replaces the entire manual proxy string with HTTP/HTTPS-only mappings,
/// including global addresses and explicit non-HTTP mappings. Reading the current
/// configuration must still succeed. Existing bypass rules and inactive settings
/// are preserved except for the proxy endpoints being replaced.
/// There is no persistent backup or rollback.
pub fn enable(forced: bool) -> Result<()> {
    update(true, forced)
}

/// Disable only HTTP/HTTPS entries pointing to Wizard. Preserve foreign entries.
pub fn disable() -> Result<()> {
    update(false, false)
}

fn update(enabled: bool, forced: bool) -> Result<()> {
    let _guard = LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("system proxy lock poisoned"))?;
    native_update(enabled, forced)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn native_status() -> Result<SystemProxyStatus> {
    let snapshot = Native::read()?;
    Ok(SystemProxyStatus {
        services: Native::services(&snapshot)?,
    })
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn native_update(enabled: bool, forced: bool) -> Result<()> {
    // Nothing of Wizard's to remove, even from a string Wizard cannot parse: uninstall
    // must not fail on foreign settings.
    #[cfg(target_os = "windows")]
    if !enabled && !Native::read()?.server.contains(wizard_proxy::ADDRESS) {
        return Ok(());
    }
    change::<Native>(enabled, forced)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_status() -> Result<SystemProxyStatus> {
    anyhow::bail!("system proxy configuration is supported only on macOS and Windows")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_update(_: bool, _: bool) -> Result<()> {
    anyhow::bail!("system proxy configuration is supported only on macOS and Windows")
}

// The boundary lets regression tests exercise the entire mutation flow without
// touching the network configuration, authorization UI or native APIs.
#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", test)),
    expect(dead_code)
)]
trait Backend {
    type Snapshot: PartialEq;
    fn read() -> Result<Self::Snapshot>;
    fn services(snapshot: &Self::Snapshot) -> Result<Vec<ServiceStatus>>;
    fn prepare(snapshot: &Self::Snapshot, enabled: bool, forced: bool) -> Result<Self::Snapshot>;
    fn write(before: &Self::Snapshot, after: &Self::Snapshot) -> Result<()>;
}

#[cfg_attr(
    not(any(target_os = "macos", target_os = "windows", test)),
    expect(dead_code)
)]
fn change<B: Backend>(enabled: bool, forced: bool) -> Result<()> {
    let (before, after) = (|| -> Result<_> {
        let before = B::read()?;
        let services = B::services(&before)?;
        ensure!(!services.is_empty(), "no supported network services");
        ensure!(
            !enabled || forced || !services.iter().any(|s| s.conflict),
            "active system proxy, PAC or auto-discovery conflicts with Wizard"
        );
        let after = B::prepare(&before, enabled, forced)?;
        // Preflight the full result before any write, including on other services.
        if enabled {
            ensure!(
                !B::services(&after)?.iter().any(|s| s.conflict),
                "other proxy protocols or unknown settings conflict with Wizard"
            );
        }
        if before != after {
            ensure!(
                B::read()? == before,
                "system proxy settings changed; try again"
            );
        }
        Ok((before, after))
    })()
    .context("system proxy settings were not changed")?;
    if before == after {
        return Ok(());
    }
    B::write(&before, &after).map_err(|error| {
        if error.is::<AuthorizationRequired>() {
            error
        } else {
            error.context("system proxy update failed; settings may be partially applied")
        }
    })?;
    ensure!(
        B::read().context("cannot verify system proxy update; settings may already be applied")?
            == after,
        "system proxy verification failed; settings may be partially applied or changed externally"
    );
    Ok(())
}

fn endpoint_state(enabled: bool, endpoint: &str) -> ProxyState {
    if !enabled {
        ProxyState::Disabled
    } else if endpoint == wizard_proxy::ADDRESS {
        ProxyState::Wizard
    } else {
        ProxyState::Foreign
    }
}
