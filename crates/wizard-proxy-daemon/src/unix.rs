use tokio::signal::unix::{SignalKind, signal};

// Install handlers now, before run_managed starts loading settings and binding.
pub(super) fn shutdown_signal() -> Result<impl Future<Output = ()>, &'static str> {
    let mut interrupt = signal(SignalKind::interrupt())
        .inspect_err(|error| wizard_proxy::diagnostics::error("SIGINT handler registration", error))
        .map_err(|_| "failed to register SIGINT handler")?;
    let mut terminate = signal(SignalKind::terminate())
        .inspect_err(|error| {
            wizard_proxy::diagnostics::error("SIGTERM handler registration", error);
        })
        .map_err(|_| "failed to register SIGTERM handler")?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => log::info!("proxy daemon received SIGINT"),
            _ = terminate.recv() => log::info!("proxy daemon received SIGTERM"),
        }
    })
}
