use super::*;
use crate::windows_config::{Config, DIRECT, PROXY};
use core::cell::RefCell;

fn disabled() -> Config {
    Config {
        flags: DIRECT,
        server: String::new(),
        bypass: "localhost;<local>".into(),
        pac: "https://example.invalid/proxy.pac".into(),
    }
}

#[derive(Default)]
struct MockState {
    snapshot: Vec<Config>,
    reads: usize,
    writes: usize,
    concurrent_change: bool,
    partial_failure: bool,
    ignore_write: bool,
}

thread_local! { static MOCK: RefCell<MockState> = RefCell::new(MockState::default()); }
struct Mock;
impl Backend for Mock {
    type Snapshot = Vec<Config>;
    fn read() -> Result<Self::Snapshot> {
        MOCK.with_borrow_mut(|m| {
            m.reads += 1;
            if m.concurrent_change && m.reads == 2 {
                m.snapshot[0].bypass = "changed".into();
            }
            Ok(m.snapshot.clone())
        })
    }
    fn services(snapshot: &Self::Snapshot) -> Result<Vec<ServiceStatus>> {
        snapshot.iter().map(Config::status).collect()
    }
    fn prepare(snapshot: &Self::Snapshot, enabled: bool, forced: bool) -> Result<Self::Snapshot> {
        snapshot
            .iter()
            .map(|c| c.prepare(enabled, forced))
            .collect()
    }
    fn write(_: &Self::Snapshot, after: &Self::Snapshot) -> Result<()> {
        MOCK.with_borrow_mut(|m| {
            m.writes += 1;
            if m.partial_failure {
                m.snapshot[0] = after[0].clone();
                anyhow::bail!("fixture write failure");
            }
            if !m.ignore_write {
                m.snapshot.clone_from(after);
            }
            Ok(())
        })
    }
}

fn setup(configs: Vec<Config>) {
    MOCK.with_borrow_mut(|m| {
        *m = MockState {
            snapshot: configs,
            ..MockState::default()
        }
    });
}

#[test]
fn preflights_every_service_before_any_write() {
    for flags in [DIRECT | 4, DIRECT | 8, DIRECT | PROXY] {
        let mut conflict = disabled();
        conflict.flags = flags;
        conflict.server = "http=foreign.example:8080".into();
        setup(vec![disabled(), conflict]);
        let error = change::<Mock>(true, false).expect_err("preflight must reject conflicts");
        let message = format!("{error:#}");
        assert!(message.contains("settings were not changed"));
        assert!(message.contains("conflicts with Wizard"));
        assert!(!message.contains("partially applied"));
        MOCK.with_borrow(|m| assert_eq!(m.writes, 0));
    }
}

#[test]
fn no_services_and_concurrent_changes_never_write() {
    for forced in [false, true] {
        setup(vec![]);
        assert!(change::<Mock>(true, forced).is_err());
        setup(vec![disabled()]);
        MOCK.with_borrow_mut(|m| m.concurrent_change = true);
        assert!(change::<Mock>(true, forced).is_err());
        MOCK.with_borrow(|m| assert_eq!(m.writes, 0));
    }
}

#[test]
fn repeated_enable_disable_are_noops_and_preserve_bypass_and_pac() -> Result<()> {
    for forced in [false, true] {
        setup(vec![disabled()]);
        change::<Mock>(true, forced)?;
        change::<Mock>(true, forced)?;
        MOCK.with_borrow(|m| {
            assert_eq!(m.writes, 1);
            assert_eq!(m.snapshot[0].bypass, disabled().bypass);
            assert_eq!(m.snapshot[0].pac, disabled().pac);
        });
        change::<Mock>(false, false)?;
        change::<Mock>(false, false)?;
        MOCK.with_borrow(|m| assert_eq!(m.writes, 2));
    }
    Ok(())
}

#[test]
fn partial_failures_and_failed_verification_are_reported_without_retry() {
    for forced in [false, true] {
        setup(vec![disabled(), disabled()]);
        MOCK.with_borrow_mut(|m| m.partial_failure = true);
        let error = change::<Mock>(true, forced).expect_err("partial application must fail");
        assert!(error.to_string().contains("partially applied"));
        assert!(!format!("{error:#}").contains("settings were not changed"));
        MOCK.with_borrow(|m| {
            assert_eq!(m.writes, 1);
            assert!(m.snapshot[0].flags & PROXY != 0);
            assert_eq!(m.snapshot[1].flags & PROXY, 0);
        });
        setup(vec![disabled()]);
        MOCK.with_borrow_mut(|m| m.ignore_write = true);
        assert!(change::<Mock>(true, forced).is_err());
        MOCK.with_borrow(|m| assert_eq!(m.writes, 1));
    }
}

#[test]
fn disabling_preserves_foreign_protocols_and_auto_modes() -> Result<()> {
    let mut config = disabled();
    config.flags |= PROXY | 4 | 8;
    config.server = format!(
        "http={};https=foreign.example:443;socks=foreign.example:1080",
        wizard_proxy::CLAUDE_DESKTOP_ADDRESS
    );
    let after = config.prepare(false, false)?;
    assert_eq!(after.flags, config.flags);
    assert_eq!(
        after.server,
        "https=foreign.example:443;socks=foreign.example:1080"
    );
    assert_eq!(after.pac, config.pac);
    assert_eq!(after.bypass, config.bypass);
    assert!(after.status()?.conflict);
    assert!(after.prepare(false, false)? == after);
    Ok(())
}

#[test]
fn global_address_is_supported_but_not_confused_with_similar_endpoints() -> Result<()> {
    for server in [
        "proxy.example:8080",
        "127.0.0.1:1",
        "http://127.0.0.1:33787",
        "127.0.0.1:337870",
    ] {
        let mut config = disabled();
        config.flags |= PROXY;
        config.server = server.into();
        assert!(config.status()?.conflict);
        assert!(config.prepare(false, false)? == config);
    }
    let mut config = disabled();
    config.flags |= PROXY;
    config.server = wizard_proxy::CLAUDE_DESKTOP_ADDRESS.into();
    assert!(!config.status()?.conflict);
    assert_eq!(config.prepare(false, false)?.flags, DIRECT);
    Ok(())
}
