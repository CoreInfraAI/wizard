//! Native `SystemConfiguration` adapter. Only this module crosses the FFI boundary.
#![expect(
    unsafe_code,
    reason = "CoreFoundation and authorization APIs require checked FFI calls"
)]

use crate::{AuthorizationRequired, Backend, ProxyState, ServiceStatus, endpoint_state};
use anyhow::{Context as _, Result, ensure};
use core::{net::SocketAddr, ptr};
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType, Type as _};
use objc2_security::{
    AuthorizationCreate, AuthorizationFlags, AuthorizationFree, AuthorizationItem,
    AuthorizationItemSet, AuthorizationRef, errAuthorizationCanceled, errAuthorizationDenied,
    errAuthorizationInteractionNotAllowed,
};
use objc2_system_configuration::{
    SCError, SCNetworkService, SCNetworkSet, SCPreferences, kSCEntNetProxies,
    kSCNetworkInterfaceTypeEthernet, kSCNetworkInterfaceTypeIEEE80211, kSCStatusAccessError,
};

pub(super) struct Native;

#[derive(Clone, PartialEq)]
pub(super) struct Snapshot {
    location: String,
    services: Vec<Service>,
}

#[derive(Clone, PartialEq)]
struct Service {
    id: String,
    config: CFRetained<CFDictionary>,
}

fn preferences() -> Result<CFRetained<SCPreferences>> {
    SCPreferences::new(None, &CFString::from_str("Wizard system proxy"), None)
        .context("cannot open system network preferences")
}

fn read_preferences(prefs: &SCPreferences) -> Result<Snapshot> {
    let set = SCNetworkSet::current(prefs).context("no current network location")?;
    let location = set
        .set_id()
        .context("network location has no identifier")?
        .to_string();
    let all = set
        .services()
        .context("cannot enumerate network services")?;
    // SystemConfiguration returns a CFArray of SCNetworkService objects.
    let all = unsafe { all.cast_unchecked::<SCNetworkService>() };
    let mut services = Vec::new();
    for service in all.iter() {
        if !service.enabled() {
            continue;
        }
        let interface = service
            .interface()
            .context("network service has no interface")?;
        let kind = interface
            .interface_type()
            .context("network interface has no type")?;
        // These exported constants are immutable framework-owned CFStrings.
        if unsafe {
            &*kind != kSCNetworkInterfaceTypeEthernet && &*kind != kSCNetworkInterfaceTypeIEEE80211
        } {
            continue;
        }
        let id = service
            .service_id()
            .context("network service has no identifier")?
            .to_string();
        let protocol = service
            .protocol(unsafe { kSCEntNetProxies })
            .context("network service has no proxy protocol")?;
        ensure!(
            protocol.enabled(),
            "network service proxy protocol is disabled"
        );
        let config = protocol
            .configuration()
            .unwrap_or_else(|| CFDictionary::<CFType, CFType>::empty().as_opaque().retain());
        services.push(Service { id, config });
    }
    services.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Snapshot { location, services })
}

fn value(config: &CFDictionary, key: &str) -> Option<CFRetained<CFType>> {
    // SC proxy dictionaries contain CF property-list keys and values.
    unsafe { config.cast_unchecked::<CFType, CFType>() }.get(&CFString::from_str(key))
}

fn number(config: &CFDictionary, key: &str) -> Result<i64> {
    let Some(value) = value(config, key) else {
        return Ok(0);
    };
    value
        .downcast_ref::<CFNumber>()
        .and_then(CFNumber::as_i64)
        .context("invalid numeric proxy setting")
}

fn state(config: &CFDictionary, prefix: &str) -> Result<ProxyState> {
    if number(config, &format!("{prefix}Enable"))? == 0 {
        return Ok(ProxyState::Disabled);
    }
    let host = value(config, &format!("{prefix}Proxy")).context("enabled proxy has no host")?;
    let host = host
        .downcast_ref::<CFString>()
        .context("invalid proxy host")?;
    let port = number(config, &format!("{prefix}Port"))?;
    Ok(endpoint_state(true, &format!("{host}:{port}")))
}

fn service_status(service: &Service) -> Result<ServiceStatus> {
    let config = &service.config;
    let http = state(config, "HTTP")?;
    let https = state(config, "HTTPS")?;
    let mut conflict = http == ProxyState::Foreign || https == ProxyState::Foreign;
    // FTP/RTSP/Gopher are legacy, but must not silently coexist with our setting.
    for key in [
        "SOCKSEnable",
        "FTPEnable",
        "RTSPEnable",
        "GopherEnable",
        "ProxyAutoConfigEnable",
        "ProxyAutoDiscoveryEnable",
    ] {
        conflict |= number(config, key)? != 0;
    }
    Ok(ServiceStatus {
        id: service.id.clone(),
        http,
        https,
        conflict,
    })
}

fn replacing(config: &CFDictionary, key: &str, value: &CFType) -> CFRetained<CFDictionary> {
    // A fresh dictionary preserves every unrelated property-list value and never
    // mutates the original snapshot returned by SystemConfiguration.
    let dict = unsafe { config.cast_unchecked::<CFType, CFType>() };
    let (mut keys, mut values) = dict.to_vecs();
    let key = CFString::from_str(key);
    let key_type: &CFType = &key;
    if let Some(index) = keys.iter().position(|k| **k == *key_type) {
        values[index] = value.retain();
    } else {
        keys.push(key_type.retain());
        values.push(value.retain());
    }
    CFDictionary::from_slices(
        &keys.iter().map(|k| &**k).collect::<Vec<_>>(),
        &values.iter().map(|v| &**v).collect::<Vec<_>>(),
    )
    .as_opaque()
    .retain()
}

impl Backend for Native {
    type Snapshot = Snapshot;

    fn read() -> Result<Snapshot> {
        let prefs = preferences()?;
        read_preferences(&prefs)
    }

    fn services(snapshot: &Snapshot) -> Result<Vec<ServiceStatus>> {
        snapshot.services.iter().map(service_status).collect()
    }

    fn prepare(snapshot: &Snapshot, enabled: bool, forced: bool) -> Result<Snapshot> {
        let address: SocketAddr = wizard_proxy::ADDRESS.parse()?;
        let host = CFString::from_str(&address.ip().to_string());
        let port = CFNumber::new_i32(i32::from(address.port()));
        let mut after = snapshot.clone();
        for service in &mut after.services {
            if enabled && forced {
                // Every setting that service_status reports as a conflict.
                for key in [
                    "SOCKSEnable",
                    "FTPEnable",
                    "RTSPEnable",
                    "GopherEnable",
                    "ProxyAutoConfigEnable",
                    "ProxyAutoDiscoveryEnable",
                ] {
                    service.config = replacing(&service.config, key, &CFNumber::new_i32(0));
                }
            }
            for prefix in ["HTTP", "HTTPS"] {
                if enabled {
                    service.config = replacing(&service.config, &format!("{prefix}Proxy"), &host);
                    service.config = replacing(&service.config, &format!("{prefix}Port"), &port);
                    service.config = replacing(
                        &service.config,
                        &format!("{prefix}Enable"),
                        &CFNumber::new_i32(1),
                    );
                } else if state(&service.config, prefix)? == ProxyState::Wizard {
                    service.config = replacing(
                        &service.config,
                        &format!("{prefix}Enable"),
                        &CFNumber::new_i32(0),
                    );
                }
            }
        }
        Ok(after)
    }

    fn write(before: &Snapshot, after: &Snapshot) -> Result<()> {
        // No authorization is requested until common preflight has succeeded.
        let authorization = Authorization::new()?;
        // Authorization stays alive until after preferences and its lock drop.
        let prefs = unsafe {
            SCPreferences::with_authorization(
                None,
                &CFString::from_str("Wizard system proxy"),
                None,
                authorization.0,
            )
        }
        .ok_or_else(|| {
            authorization_preferences_error("cannot open authorized network preferences", SCError())
        })?;
        if !prefs.lock(false) {
            return Err(authorization_preferences_error(
                "cannot lock network preferences",
                SCError(),
            ));
        }
        let _lock = PreferencesLock(&prefs);
        ensure!(
            read_preferences(&prefs)? == *before,
            "network settings changed during authorization; try again"
        );
        for (old, new) in before.services.iter().zip(&after.services) {
            if old == new {
                continue;
            }
            let service = SCNetworkService::copy(&prefs, &CFString::from_str(&new.id))
                .context("network service disappeared")?;
            let protocol = service
                .protocol(unsafe { kSCEntNetProxies })
                .context("proxy protocol disappeared")?;
            // The snapshot is an owned property-list dictionary valid for this call.
            ensure!(
                unsafe { protocol.set_configuration(Some(&new.config)) },
                "cannot stage proxy configuration"
            );
        }
        ensure!(prefs.commit_changes(), "cannot commit network preferences");
        ensure!(
            prefs.apply_changes(),
            "network preferences saved but could not be applied"
        );
        Ok(())
    }
}

// Only used before staging or writing any changes: an access failure here
// reliably means the caller can retry authorization without partial writes.
fn authorization_preferences_error(operation: &'static str, code: i32) -> anyhow::Error {
    if u32::try_from(code).ok() == Some(kSCStatusAccessError) {
        AuthorizationRequired { code }.into()
    } else {
        anyhow::anyhow!("{operation} (SystemConfiguration code {code})")
    }
}

fn check_authorization(code: i32) -> Result<()> {
    if [
        errAuthorizationDenied,
        errAuthorizationCanceled,
        errAuthorizationInteractionNotAllowed,
    ]
    .contains(&code)
    {
        return Err(AuthorizationRequired { code }.into());
    }
    ensure!(
        code == 0,
        "network configuration authorization failed ({code})"
    );
    Ok(())
}

struct PreferencesLock<'a>(&'a SCPreferences);
impl Drop for PreferencesLock<'_> {
    fn drop(&mut self) {
        self.0.unlock();
    }
}

struct Authorization(AuthorizationRef);
impl Authorization {
    fn new() -> Result<Self> {
        let mut item = AuthorizationItem {
            name: c"system.preferences.network".as_ptr(),
            valueLength: 0,
            value: ptr::null_mut(),
            flags: 0,
        };
        let rights = AuthorizationItemSet {
            count: 1,
            items: &raw mut item,
        };
        let mut authorization = ptr::null();
        // All pointers refer to live stack values; the API returns an owned ref.
        let code = unsafe {
            AuthorizationCreate(
                &raw const rights,
                ptr::null(),
                AuthorizationFlags::InteractionAllowed | AuthorizationFlags::ExtendRights,
                &raw mut authorization,
            )
        };
        check_authorization(code)?;
        ensure!(
            !authorization.is_null(),
            "network configuration authorization returned no reference"
        );
        Ok(Self(authorization))
    }
}
impl Drop for Authorization {
    fn drop(&mut self) {
        // Release our reference without revoking shared authorization rights.
        unsafe {
            AuthorizationFree(self.0, AuthorizationFlags::Defaults);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorization_errors_are_not_confused_with_lock_contention() {
        for code in [
            errAuthorizationDenied,
            errAuthorizationCanceled,
            errAuthorizationInteractionNotAllowed,
        ] {
            let error = check_authorization(code).expect_err("authorization was not granted");
            assert_eq!(
                error
                    .downcast_ref::<AuthorizationRequired>()
                    .expect("typed authorization error")
                    .code,
                code
            );
        }
        assert!(check_authorization(0).is_ok());
        assert!(
            !check_authorization(-60001)
                .expect_err("invalid authorization reference")
                .is::<AuthorizationRequired>()
        );
        let error = authorization_preferences_error("fixture lock", 1003);
        assert!(error.is::<AuthorizationRequired>());
        let error = authorization_preferences_error("fixture lock", 1006);
        assert!(!error.is::<AuthorizationRequired>());
        assert!(error.to_string().contains("SystemConfiguration code 1006"));
    }

    #[test]
    fn preserves_unrelated_properties_and_disables_only_our_protocol() -> Result<()> {
        let empty = CFDictionary::<CFType, CFType>::empty();
        let config = replacing(
            empty.as_opaque(),
            "ExceptionsList",
            &CFString::from_str("fixture"),
        );
        let before = Snapshot {
            location: "fixture".into(),
            services: vec![Service {
                id: "fixture".into(),
                config,
            }],
        };
        let enabled = Native::prepare(&before, true, false)?;
        assert_eq!(Native::services(&enabled)?[0].http, ProxyState::Wizard);
        assert_eq!(
            value(&before.services[0].config, "ExceptionsList"),
            value(&enabled.services[0].config, "ExceptionsList")
        );
        let mut foreign = enabled;
        foreign.services[0].config = replacing(
            &foreign.services[0].config,
            "HTTPSProxy",
            &CFString::from_str("foreign.example"),
        );
        let disabled = Native::prepare(&foreign, false, false)?;
        let status = Native::services(&disabled)?;
        assert_eq!(status[0].http, ProxyState::Disabled);
        assert_eq!(status[0].https, ProxyState::Foreign);
        assert!(status[0].conflict);
        Ok(())
    }

    #[test]
    fn forced_mode_clears_every_reported_conflict() -> Result<()> {
        let mut config = CFDictionary::<CFType, CFType>::empty().as_opaque().retain();
        for key in ["SOCKSEnable", "FTPEnable", "ProxyAutoConfigEnable"] {
            config = replacing(&config, key, &CFNumber::new_i32(1));
        }
        let before = Snapshot {
            location: "fixture".into(),
            services: vec![Service {
                id: "fixture".into(),
                config,
            }],
        };
        assert!(Native::services(&before)?[0].conflict);
        let normal = Native::prepare(&before, true, false)?;
        assert!(Native::services(&normal)?[0].conflict);
        let forced = Native::prepare(&before, true, true)?;
        let status = &Native::services(&forced)?[0];
        assert!(!status.conflict);
        assert_eq!(status.http, ProxyState::Wizard);
        Ok(())
    }
}
