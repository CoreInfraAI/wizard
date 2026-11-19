use super::{Backend, ProxyState, ServiceStatus};
use anyhow::{Context as _, Result, ensure};
extern crate alloc;
use alloc::collections::BTreeMap;
use std::process::Command;

pub(super) struct Native;
const ROOT: &str = "org.gnome.system.proxy";
// Order matters: activate manual mode only after all endpoints are ready.
const KEYS: [(&str, &str); 7] = [
    ("http", "use-authentication"),
    ("http", "port"),
    ("http", "host"),
    ("https", "port"),
    ("https", "host"),
    ("", "use-same-proxy"),
    ("", "mode"),
];
type Snapshot = BTreeMap<(&'static str, &'static str), String>;

fn schema(child: &str) -> String {
    if child.is_empty() {
        ROOT.to_owned()
    } else {
        format!("{ROOT}.{child}")
    }
}

fn parse(output: &str) -> Result<Snapshot> {
    let mut values = Snapshot::new();
    for key in KEYS {
        let prefix = format!("{} {} ", schema(key.0), key.1);
        let value = output
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .context("missing GNOME proxy setting")?;
        values.insert(key, value.to_owned());
    }
    ensure!(
        matches!(
            values[&("", "mode")].as_str(),
            "'auto'" | "'none'" | "'manual'"
        ),
        "unknown GNOME proxy mode"
    );
    Ok(values)
}

fn endpoint(s: &Snapshot, protocol: &'static str) -> ProxyState {
    let host = &s[&(protocol, "host")];
    if host == "''" {
        return ProxyState::Disabled;
    }
    let (ours, port) = wizard_proxy::ADDRESS
        .split_once(':')
        .expect("IPv4 endpoint");
    if host == &format!("'{ours}'") && s[&(protocol, "port")] == port {
        ProxyState::Wizard
    } else {
        ProxyState::Foreign
    }
}

impl Backend for Native {
    type Snapshot = Snapshot;
    fn read() -> Result<Snapshot> {
        ensure!(super::supported(), "not a GNOME session");
        let out = Command::new("/usr/bin/gsettings")
            .args(["list-recursively", ROOT])
            .output()?;
        ensure!(out.status.success(), "cannot read GNOME proxy settings");
        parse(core::str::from_utf8(&out.stdout).context("invalid GNOME proxy settings output")?)
    }

    fn services(s: &Snapshot) -> Result<Vec<ServiceStatus>> {
        let mode = &s[&("", "mode")];
        let configured = [endpoint(s, "http"), endpoint(s, "https")];
        let [http, https] = if mode == "'manual'" {
            configured
        } else {
            [ProxyState::Disabled; 2]
        };
        // Dormant manual settings would become active or be overwritten on enable.
        let conflict = mode == "'auto'"
            || configured.contains(&ProxyState::Foreign)
            || s[&("http", "use-authentication")] != "false"
            || s[&("", "use-same-proxy")] != "false";
        Ok(vec![ServiceStatus {
            id: "GNOME".into(),
            http,
            https,
            conflict,
        }])
    }

    fn prepare(s: &Snapshot, enabled: bool, forced: bool) -> Result<Snapshot> {
        let mut after = s.clone();
        let (host, port) = wizard_proxy::ADDRESS
            .split_once(':')
            .expect("IPv4 endpoint");
        if enabled {
            if forced {
                for key in [("http", "use-authentication"), ("", "use-same-proxy")] {
                    after.insert(key, "false".into());
                }
            }
            for protocol in ["http", "https"] {
                after.insert((protocol, "host"), format!("'{host}'"));
                after.insert((protocol, "port"), port.into());
            }
            after.insert(("", "mode"), "'manual'".into());
        } else if s[&("", "mode")] == "'manual'" {
            for protocol in ["http", "https"] {
                if endpoint(s, protocol) == ProxyState::Wizard {
                    after.insert((protocol, "host"), "''".into());
                }
            }
            // Leave the shared manual mode intact for protocols we do not manage.
        }
        Ok(after)
    }

    fn write(before: &Snapshot, after: &Snapshot) -> Result<()> {
        for key in KEYS {
            if before[&key] != after[&key] {
                let out = Command::new("/usr/bin/gsettings")
                    .args(["set", &schema(key.0), key.1, &after[&key]])
                    .output()?;
                ensure!(out.status.success(), "cannot write GNOME proxy setting");
            }
        }
        Ok(())
    }
}
