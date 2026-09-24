// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Err(error) = wizard_lib::run_application() {
        eprintln!("application failed: {error:#}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
