use super::*;
use alloc::sync::Arc;
use std::sync::Barrier;
use wizard_system_proxy::ServiceStatus;

fn snapshot(http: ProxyState, https: ProxyState, conflict: bool) -> SystemProxyStatus {
    SystemProxyStatus {
        services: vec![ServiceStatus {
            id: "fixture".into(),
            http,
            https,
            conflict,
        }],
    }
}

#[test]
fn aggregates_os_settings_without_treating_a_stopped_listener_as_a_conflict() {
    use ProxyState::{Disabled, Foreign, Wizard};
    for (http, https, conflict, status, can_toggle) in [
        (Disabled, Disabled, false, Status::Off, true),
        (Foreign, Foreign, true, Status::Off, false),
        (Wizard, Wizard, false, Status::On, true),
        (Wizard, Wizard, true, Status::Partial, true),
        (Wizard, Foreign, true, Status::Partial, true),
        (Wizard, Disabled, false, Status::Partial, true),
    ] {
        let mut state = State::default();
        observe(&mut state, Ok(snapshot(http, https, conflict)), false);
        assert_eq!(state.status, status);
        assert_eq!(state.can_toggle(), can_toggle);
    }
    let mut state = State::default();
    let mut mixed = snapshot(Wizard, Wizard, false);
    mixed
        .services
        .extend(snapshot(Disabled, Disabled, false).services);
    observe(&mut state, Ok(mixed), true);
    assert_eq!(state.status, Status::Partial);
    observe(&mut state, Ok(SystemProxyStatus { services: vec![] }), true);
    assert_eq!(state.status, Status::Unavailable);
    assert!(!state.can_toggle());

    // External OS changes must not erase saved demand or prevent cancelling it.
    state.requested = true;
    observe(&mut state, Ok(snapshot(Foreign, Foreign, true)), false);
    assert_eq!(state.status, Status::Off);
    assert!(state.requested);
    assert!(state.can_toggle());
}

#[test]
fn failed_read_is_unknown_and_polling_preserves_operation_errors() {
    let mut state = State {
        operation_error: Some("fixture operation failure".into()),
        ..State::default()
    };
    observe(
        &mut state,
        Err(anyhow::anyhow!("fixture read failure")),
        true,
    );
    assert_eq!(state.status, Status::Unknown);
    assert!(state.read_error.is_some());
    assert!(!state.can_toggle());
    observe(
        &mut state,
        Ok(snapshot(ProxyState::Wizard, ProxyState::Wizard, false)),
        true,
    );
    assert_eq!(state.status, Status::On);
    assert!(state.read_error.is_none());
    assert!(state.operation_error.is_some());
    let signal = RevisionSignal::new(state);
    assert!(begin(&signal).is_ok());
    assert!(signal.current().state.operation_error.is_none());
}

#[test]
fn late_observation_cannot_clear_busy_or_admit_another_mutation() {
    let signal = RevisionSignal::new(State::default());
    let read_before_mutation = snapshot(ProxyState::Disabled, ProxyState::Disabled, false);
    assert!(begin(&signal).is_ok());
    update(&signal, |state| {
        observe(state, Ok(read_before_mutation), true);
    });
    assert!(signal.current().state.busy);
    assert!(!signal.current().state.can_toggle());
    assert!(begin(&signal).is_err());
    update(&signal, |state| state.busy = false);
    assert!(begin(&signal).is_ok());
}

#[test]
fn simultaneous_clicks_admit_only_one_mutation() {
    let signal = Arc::new(RevisionSignal::new(State::default()));
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let signal = Arc::clone(&signal);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                begin(&signal).is_ok()
            })
        })
        .collect();
    let accepted = handles
        .into_iter()
        .map(|h| h.join().expect("admission worker"))
        .filter(|&ok| ok)
        .count();
    assert_eq!(accepted, 1);
}

#[test]
fn unchanged_observations_do_not_publish() {
    let signal = RevisionSignal::new(State::default());
    for _ in 0..2 {
        update(&signal, |state| {
            observe(
                state,
                Ok(snapshot(ProxyState::Wizard, ProxyState::Wizard, false)),
                true,
            );
        });
    }
    assert_eq!(signal.current().revision, 1);
}

#[test]
fn authorization_failure_has_a_specific_retry_message() {
    let observed = State {
        status: Status::Off,
        ..State::default()
    };
    let error = anyhow::Error::new(os::AuthorizationRequired { code: 1003 });
    let errors = os_errors(Err(error), &observed, true);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("Подтвердите запрос администратора"));
    assert!(errors[0].contains("Настройки ОС не изменены"));
    assert!(!errors[0].contains("частично"));
    assert!(!errors[0].contains("partially applied"));

    let error = anyhow::anyhow!("cannot lock network preferences (SystemConfiguration code 1006)");
    let errors = os_errors(Err(error), &observed, true);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("1006"));
    assert!(!errors[0].contains("Подтвердите запрос администратора"));
}

#[test]
fn os_errors_distinguish_preflight_partial_write_and_observation_failures() {
    let off = State {
        status: Status::Off,
        ..State::default()
    };
    let rejected =
        anyhow::anyhow!("fixture conflict").context("system proxy settings were not changed");
    let errors = os_errors(Err(rejected), &off, true);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("settings were not changed"));
    assert!(errors[0].contains("fixture conflict"));
    assert!(!errors[0].contains("частично"));
    assert!(!errors[0].contains("partially applied"));

    let partial = anyhow::anyhow!("fixture write failure")
        .context("system proxy update failed; settings may be partially applied");
    let errors = os_errors(Err(partial), &off, true);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("partially applied"));
    assert!(!errors[0].contains("settings were not changed"));

    let unknown = State {
        read_error: Some("fixture observation failure".into()),
        ..State::default()
    };
    assert_eq!(
        os_errors(Ok(()), &unknown, true),
        ["fixture observation failure"]
    );
    let errors = os_errors(
        Err(anyhow::anyhow!("fixture write failure")),
        &unknown,
        true,
    );
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[1], "fixture observation failure");
    assert_eq!(os_errors(Ok(()), &off, true).len(), 1);
    assert!(os_errors(Ok(()), &off, false).is_empty());
}

thread_local! {
    static LOG_RECORDS: core::cell::RefCell<Option<Vec<String>>> = const { core::cell::RefCell::new(None) };
}

struct TestLogger;
impl log::Log for TestLogger {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        LOG_RECORDS.with_borrow_mut(|records| {
            if let Some(records) = records {
                records.push(format!("{} {}", record.level(), record.args()));
            }
        });
    }
    fn flush(&self) {}
}

#[test]
fn logs_safe_failures_without_hub_details_or_repeated_poll_errors() {
    static LOGGER: TestLogger = TestLogger;
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        log::set_logger(&LOGGER).expect("test logger");
        log::set_max_level(log::LevelFilter::Info);
    });
    LOG_RECORDS.with_borrow_mut(|records| *records = Some(Vec::new()));

    log_operation_result(
        true,
        true,
        &Err("fixture Hub error with secret-value".into()),
    );
    log_operation_result(false, false, &Ok(()));
    let _ = os_errors(
        Err(anyhow::anyhow!("fixture OS conflict")),
        &State::default(),
        true,
    );
    let mut state = State::default();
    for _ in 0..2 {
        observe(
            &mut state,
            Err(anyhow::anyhow!("fixture OS read failure")),
            true,
        );
    }
    observe(
        &mut state,
        Ok(snapshot(ProxyState::Wizard, ProxyState::Wizard, false)),
        true,
    );

    let records = LOG_RECORDS.with_borrow_mut(|records| records.take().expect("captured logs"));
    let logs = records.join("\n");
    assert!(logs.contains("ERROR system proxy operation failed: enabled=true, forced=true"));
    assert!(logs.contains("INFO system proxy operation completed: enabled=false, forced=false"));
    assert!(logs.contains("ERROR system proxy OS update failed: fixture OS conflict"));
    assert_eq!(
        records
            .iter()
            .filter(|record| record.contains("system proxy observation failed"))
            .count(),
        1
    );
    assert!(logs.contains("system proxy observation recovered"));
    assert!(!logs.contains("fixture Hub error"));
    assert!(!logs.contains("secret-value"));
}
