// Prevents an additional console window on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some(arg1) = std::env::args_os().nth(1)
        && arg1 == "--proxy"
    {
        return wizard_proxy_daemon::main();
    }

    if let Err(error) = wizard_gui::run_application() {
        eprintln!("application failed: {error:#}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
