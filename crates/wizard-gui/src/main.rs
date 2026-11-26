// Prevents an additional console window on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let first = args.next();
    if first.as_deref() == Some(wizard_core::agents::claude_desktop::credential::ARG.as_ref()) {
        let (Some(id), Some(digest), None) = (args.next(), args.next(), args.next()) else {
            eprintln!("Expected the Claude Desktop profile ID and credential fingerprint");
            return ExitCode::FAILURE;
        };
        let (Some(id), Some(digest)) = (id.to_str(), digest.to_str()) else {
            eprintln!("Claude Desktop credential arguments must be valid UTF-8");
            return ExitCode::FAILURE;
        };
        return match wizard_core::agents::claude_desktop::credential::run(id, digest) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Claude Desktop credential: {error:#}");
                ExitCode::FAILURE
            }
        };
    }
    if first
        .as_deref()
        .is_some_and(wizard_core::media::stdio::is_arg)
    {
        let port = args
            .next()
            .and_then(|arg| arg.to_str()?.parse::<u16>().ok());
        let (Some(port), Some(path), None) = (port, args.next(), args.next()) else {
            eprintln!("Expected the Wizard port and media headers file");
            return ExitCode::FAILURE;
        };
        return match tauri::async_runtime::block_on(wizard_core::media::stdio::run(
            port,
            path.into(),
        )) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Media MCP: {error:#}");
                ExitCode::FAILURE
            }
        };
    }
    if first.as_deref() == Some(std::ffi::OsStr::new("--proxy")) {
        return wizard_proxy_daemon::main(wizard_core::media::background::Service::default());
    }

    if first.as_deref() == Some(std::ffi::OsStr::new("--uninstall-proxy")) {
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
