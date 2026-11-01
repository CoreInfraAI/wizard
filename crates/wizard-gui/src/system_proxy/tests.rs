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
