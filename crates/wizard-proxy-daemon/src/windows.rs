use core::{ptr, time::Duration};
use std::{
    ffi::OsString,
    io::{self, Write as _},
    os::windows::{
        ffi::OsStringExt as _,
        io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle},
    },
    path::PathBuf,
    process::Stdio,
};
use tokio::process::Command;
use windows_sys::Win32::{
    Foundation::{ERROR_INVALID_PARAMETER, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        SystemInformation::GetSystemDirectoryW,
        Threading::{CREATE_NO_WINDOW, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    },
};

pub(super) async fn create_daemon() -> io::Result<()> {
    log::info!("requesting graceful proxy shutdown before task update");
    match wizard_proxy::control::shutdown().await {
        Ok(response) => {
            tokio::task::spawn_blocking(move || wait_for_exit(response.pid))
                .await
                .inspect_err(|error| {
                    wizard_proxy::diagnostics::error("proxy process exit worker", error);
                })
                .map_err(|_| io::Error::other("proxy process exit worker failed"))??;
        }
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(error) => return Err(error),
    }

    let executable = std::env::current_exe()?;
    // The caller must be the executable supporting --proxy, even if renamed.
    let executable = executable.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "proxy executable path is not valid Unicode",
        )
    })?;
    let sid = current_user_sid().await?;
    let xml = super::windows_task::definition(&sid, executable)?;
    // UTF-16LE with BOM makes the file encoding explicit to schtasks.
    let bytes: Vec<u8> = core::iter::once(0xfeff)
        .chain(xml.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect();
    let mut file = tempfile::Builder::new()
        .prefix("wizard-proxy-")
        .suffix(".xml")
        .tempfile()?;
    file.write_all(&bytes)?;
    // schtasks opens XML without write sharing. Close the write handle while
    // retaining the path guard so the temporary file is removed after import.
    let path = file.into_temp_path();

    let mut command = Command::new(system_executable("schtasks.exe")?);
    command
        .args(["/Create", "/TN", &task_name(&sid), "/XML"])
        .arg(&path)
        .arg("/F");
    log::info!("registering current user's proxy task");
    run_schtasks(&mut command, "registration").await
}

pub(super) async fn uninstall() -> io::Result<()> {
    let sid = current_user_sid().await?;
    let executable = std::env::current_exe()?;
    if !update_uninstall_task(&sid, &executable, "disable").await? {
        // Without a task we cannot establish installation ownership. Do not
        // send shutdown to an arbitrary process on the management port.
        let _instance = super::acquire_instance_lock().map_err(io::Error::other)?;
        return Ok(());
    }

    match wizard_proxy::control::status().await {
        Ok(_) => {
            let response = wizard_proxy::control::shutdown().await?;
            tokio::task::spawn_blocking(move || wait_for_exit(response.pid))
                .await
                .map_err(|_| io::Error::other("proxy exit worker failed"))??;
        }
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {}
        Err(error) => return Err(error),
    }

    let _instance = super::acquire_instance_lock().map_err(io::Error::other)?;
    update_uninstall_task(&sid, &executable, "delete").await?;
    Ok(())
}

async fn update_uninstall_task(
    sid: &str,
    executable: &std::path::Path,
    operation: &str,
) -> io::Result<bool> {
    let status = tokio::time::timeout(
        Duration::from_secs(15),
        Command::new(system_executable(r"WindowsPowerShell\v1.0\powershell.exe")?)
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
            .arg(include_str!("windows-uninstall.ps1"))
            .env("WIZARD_UNINSTALL_TASK", task_name(sid))
            .env("WIZARD_UNINSTALL_EXE", executable)
            .env("WIZARD_UNINSTALL_OPERATION", operation)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "proxy task cleanup timed out"))??;
    match status.code() {
        Some(0) => Ok(true),
        Some(10) => Ok(false),
        _ => Err(io::Error::other("proxy task cleanup failed")),
    }
}

pub(super) async fn start_daemon() -> io::Result<()> {
    let sid = current_user_sid().await?;
    let mut command = Command::new(system_executable("schtasks.exe")?);
    command.args(["/Run", "/TN", &task_name(&sid)]);
    run_schtasks(&mut command, "start").await
}

pub(super) fn shutdown_signal() -> Result<impl Future<Output = ()>, &'static str> {
    use tokio::signal::windows::{ctrl_break, ctrl_c, ctrl_close};

    let mut interrupt = ctrl_c()
        .inspect_err(|error| wizard_proxy::diagnostics::error("Ctrl+C handler registration", error))
        .map_err(|_| "failed to register Ctrl+C handler")?;
    let mut stop = ctrl_break()
        .inspect_err(|error| {
            wizard_proxy::diagnostics::error("Ctrl+Break handler registration", error);
        })
        .map_err(|_| "failed to register Ctrl+Break handler")?;
    let mut close = ctrl_close()
        .inspect_err(|error| {
            wizard_proxy::diagnostics::error("console-close handler registration", error);
        })
        .map_err(|_| "failed to register console-close handler")?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => log::info!("proxy daemon received Ctrl+C"),
            _ = stop.recv() => log::info!("proxy daemon received Ctrl+Break"),
            _ = close.recv() => log::info!("proxy daemon received console-close"),
        }
    })
}

fn task_name(sid: &str) -> String {
    format!("ai.coreinfra.wizard.proxy.{sid}")
}

async fn run_schtasks(command: &mut Command, operation: &'static str) -> io::Result<()> {
    // Dropping the timed-out future terminates the utility, but cannot roll back
    // a registration already accepted by Task Scheduler. Never retry automatically.
    let status = tokio::time::timeout(
        Duration::from_secs(5),
        command
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .status(),
    )
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("proxy task {operation} timed out"),
        )
    })??;
    if !status.success() {
        log::error!(
            "proxy task {operation} failed: exit_code={:?}",
            status.code()
        );
        return Err(io::Error::other(format!(
            "proxy task {operation} failed (exit code {:?})",
            status.code(),
        )));
    }
    Ok(())
}

#[expect(
    unsafe_code,
    reason = "wait on an owned Windows process handle without terminating it"
)]
fn wait_for_exit(pid: u32) -> io::Result<()> {
    if pid == 0 || pid == std::process::id() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid proxy shutdown PID",
        ));
    }
    // SAFETY: OpenProcess validates the PID. Request only synchronization access.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        let error = io::Error::last_os_error();
        if error.raw_os_error() == i32::try_from(ERROR_INVALID_PARAMETER).ok() {
            log::info!("proxy daemon already exited");
            return Ok(());
        }
        return Err(error);
    }
    // SAFETY: OpenProcess returned a new valid handle; transfer ownership once.
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    log::info!("waiting for proxy daemon process exit");
    // SAFETY: the owned process handle remains alive throughout this bounded wait.
    match unsafe { WaitForSingleObject(process.as_raw_handle(), 10_000) } {
        WAIT_OBJECT_0 => {
            log::info!("proxy daemon process exited");
            Ok(())
        }
        WAIT_TIMEOUT => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "proxy daemon did not exit",
        )),
        _ => Err(io::Error::last_os_error()),
    }
}

#[expect(
    unsafe_code,
    reason = "resolve the system directory without trusting PATH or environment variables"
)]
fn system_executable(name: &str) -> io::Result<PathBuf> {
    // SAFETY: a null buffer with zero capacity queries the required size.
    let required = unsafe { GetSystemDirectoryW(ptr::null_mut(), 0) };
    if required == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut buffer = vec![0_u16; required as usize];
    // SAFETY: buffer has capacity for required UTF-16 units.
    let written = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), required) };
    if written == 0 {
        return Err(io::Error::last_os_error());
    }
    if written >= required {
        return Err(io::Error::other(
            "system directory exceeded its reported size",
        ));
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..written as usize])).join(name))
}

async fn current_user_sid() -> io::Result<String> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        Command::new(system_executable("whoami.exe")?)
            .args(["/user", "/fo", "csv", "/nh"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "current user SID lookup timed out"))??;
    if !output.status.success() {
        log::error!(
            "current user SID lookup failed: exit_code={:?}",
            output.status.code()
        );
        return Err(io::Error::other(format!(
            "current user SID lookup failed (exit code {:?})",
            output.status.code(),
        )));
    }
    super::windows_task::sid_from_whoami(&output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn uninstall_task_ownership_and_idempotency() -> io::Result<()> {
        let output = tokio::time::timeout(
            Duration::from_secs(60),
            Command::new(system_executable(r"WindowsPowerShell\v1.0\powershell.exe")?)
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/windows-uninstall.ps1"
                ))
                .creation_flags(CREATE_NO_WINDOW)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "task cleanup test timed out"))??;
        assert!(
            output.status.success(),
            "task cleanup test failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
        Ok(())
    }
}
