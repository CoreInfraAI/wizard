// Prevents an additional console window on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(wizard_core::video::stdio::ARG.as_ref()) {
        let port = args
            .next()
            .and_then(|arg| arg.to_str()?.parse::<u16>().ok());
        let (Some(port), Some(path), None) = (port, args.next(), args.next()) else {
            eprintln!("Expected the Wizard port and video headers file");
            return ExitCode::FAILURE;
        };
        return match tauri::async_runtime::block_on(wizard_core::video::stdio::run(
            port,
            path.into(),
        )) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Video MCP: {error:#}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(arg1) = std::env::args_os().nth(1)
        && arg1 == "--proxy"
    {
        return wizard_proxy_daemon::main();
    }

    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--uninstall-proxy")
    {
        return wizard_proxy_daemon::uninstall();
    }

    // Shared across GUI instances; uninstall requires exclusive access.
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
