use tokio::sync::watch;

pub(crate) struct RevisionSignal {
    revision: watch::Sender<u32>,
}

impl Default for RevisionSignal {
    fn default() -> Self {
        let (revision, _) = watch::channel(0);
        Self { revision }
    }
}

impl RevisionSignal {
    pub(crate) fn current(&self) -> u32 {
        *self.revision.borrow()
    }

    pub(crate) fn notify(&self) {
        self.revision.send_modify(|revision| {
            *revision = revision.wrapping_add(1);
            log::debug!("state revision advanced to {revision}");
        });
    }

    pub(crate) async fn wait(&self, last_revision: u32) -> Result<u32, String> {
        let mut revision = self.revision.subscribe();
        loop {
            let current = *revision.borrow_and_update();
            if current != last_revision {
                return Ok(current);
            }
            revision
                .changed()
                .await
                .map_err(|error| error.to_string())?;
        }
    }
}

#[tauri::command]
pub(crate) async fn wait_for_update(
    last_revision: String,
    signal: tauri::State<'_, RevisionSignal>,
) -> Result<String, String> {
    let revision = last_revision
        .parse::<u32>()
        .map_err(|error| error.to_string())?;
    signal
        .wait(revision)
        .await
        .map(|revision| revision.to_string())
}
