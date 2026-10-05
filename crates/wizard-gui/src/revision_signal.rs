use anyhow::{Context as _, Result};
use serde::Serialize;
use tokio::sync::watch;

#[derive(Clone, Serialize)]
pub(crate) struct RevisionSnapshot<T> {
    pub(crate) revision: u32,
    #[serde(flatten)]
    pub(crate) state: T,
}

pub(crate) struct RevisionSignal<T> {
    state: watch::Sender<RevisionSnapshot<T>>,
}

impl<T: Clone> RevisionSignal<T> {
    pub(crate) fn new(state: T) -> Self {
        let (state, _) = watch::channel(RevisionSnapshot { revision: 0, state });
        Self { state }
    }

    pub(crate) fn current(&self) -> RevisionSnapshot<T> {
        self.state.borrow().clone()
    }

    pub(crate) fn notify(&self, state: T) {
        self.update(|_| Some(state));
    }

    // Returning None leaves both the state and revision unchanged.
    pub(crate) fn update(
        &self,
        update: impl FnOnce(&T) -> Option<T>,
    ) -> Option<RevisionSnapshot<T>> {
        let mut updated = None;
        self.state.send_if_modified(|snapshot| {
            let Some(state) = update(&snapshot.state) else {
                return false;
            };
            snapshot.revision = snapshot.revision.wrapping_add(1);
            snapshot.state = state;
            updated = Some(snapshot.clone());
            true
        });
        updated
    }

    pub(crate) async fn wait(&self, last_revision: Option<u32>) -> Result<RevisionSnapshot<T>> {
        let mut state = self.state.subscribe();
        loop {
            let snapshot = state.borrow_and_update().clone();
            if Some(snapshot.revision) != last_revision {
                return Ok(snapshot);
            }
            state.changed().await.context("revision signal closed")?;
        }
    }
}
