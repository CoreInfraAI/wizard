// Prevents an additional console window on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some(arg1) = std::env::args_os().nth(1)
        && arg1 == "--proxy"
    {
        return wizard_proxy_daemon::main();
    }

    #[cfg(windows)]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--uninstall-proxy")
    {
        return wizard_proxy_daemon::uninstall();
    }

    // Shared across GUI instances; uninstall requires exclusive access.
    #[cfg(windows)]
    let _uninstall_guard = match wizard_proxy_daemon::gui_guard() {
        Ok(guard) => guard,
        Err(error) => {
            eprintln!("cannot start Wizard during proxy cleanup: {error}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(error) = wizard_gui::run_application() {
        eprintln!("application failed: {error:#}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
