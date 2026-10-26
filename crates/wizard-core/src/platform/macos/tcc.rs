//! macOS charges privacy (TCC) access to a process's "responsible process", which children
//! inherit through fork, exec, `setsid` and reparenting to launchd. Daemons started by the app
//! would otherwise be attributed to Wizard for their whole lifetime.
//! The responsibility functions are private libSystem API (also used by LLDB and Chromium),
//! so they are resolved at runtime instead of linked.
#![expect(
    unsafe_code,
    reason = "posix_spawn and the private responsibility functions have no safe bindings"
)]

extern crate alloc;

use alloc::{collections::BTreeMap, ffi::CString};
use anyhow::{Context as _, Result, bail};
use core::{
    ffi::{CStr, c_char, c_int, c_short, c_void},
    time::Duration,
};
use std::{
    ffi::OsString,
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        process::ExitStatusExt as _,
    },
    process::{Command, ExitStatus},
    time::Instant,
};

type GetResponsiblePid = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
type SetDisclaim = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, c_int) -> c_int;

const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

fn symbol(name: &CStr) -> Option<*mut c_void> {
    // SAFETY: `name` is NUL-terminated; RTLD_DEFAULT only searches loaded images.
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    (!symbol.is_null()).then_some(symbol)
}

/// Reports a process itself once its responsible process has exited.
fn responsible_pid(pid: libc::pid_t) -> Option<libc::pid_t> {
    let function = symbol(c"responsibility_get_pid_responsible_for_pid")?;
    // SAFETY: matches Apple's private declaration, as used by LLDB.
    let function = unsafe { core::mem::transmute::<*mut c_void, GetResponsiblePid>(function) };
    // SAFETY: the function only reads kernel process information.
    Some(unsafe { function(pid) })
}

/// True for an app launched by launchd; false for a CLI charged to its terminal.
/// Also false when the lookup is unavailable or fails, keeping the ordinary attribution.
pub(crate) fn is_self_responsible() -> bool {
    libc::pid_t::try_from(std::process::id()).is_ok_and(|pid| responsible_pid(pid) == Some(pid))
}

struct SpawnAttributes(libc::posix_spawnattr_t);

impl Drop for SpawnAttributes {
    fn drop(&mut self) {
        // SAFETY: constructed only after a successful posix_spawnattr_init.
        unsafe { libc::posix_spawnattr_destroy(&raw mut self.0) };
    }
}

struct FileActions(libc::posix_spawn_file_actions_t);

impl Drop for FileActions {
    fn drop(&mut self) {
        // SAFETY: constructed only after a successful posix_spawn_file_actions_init.
        unsafe { libc::posix_spawn_file_actions_destroy(&raw mut self.0) };
    }
}

fn check(code: c_int, operation: &str) -> Result<()> {
    if code == 0 {
        return Ok(());
    }
    Err(std::io::Error::from_raw_os_error(code)).with_context(|| format!("{operation} failed"))
}

fn c_string(bytes: &[u8]) -> Result<CString> {
    CString::new(bytes).context("command contains a NUL byte")
}

/// Runs `command` as its own responsible process, with null stdio, and waits for it.
/// Uses only the program, arguments and environment changes.
/// Must be called inside `spawn_blocking`.
pub(crate) fn command_status_disclaimed(
    command: &Command,
    timeout: Duration,
) -> Result<ExitStatus> {
    let set_disclaim = symbol(c"responsibility_spawnattrs_setdisclaim")
        .context("this macOS version cannot start a process as its own responsible process")?;
    // SAFETY: matches Apple's private declaration, as used by LLDB.
    let set_disclaim = unsafe { core::mem::transmute::<*mut c_void, SetDisclaim>(set_disclaim) };

    let program_name = command.get_program().to_string_lossy().into_owned();
    let program = c_string(command.get_program().as_bytes())?;
    let mut arguments = vec![program.clone()];
    for argument in command.get_args() {
        arguments.push(c_string(argument.as_bytes())?);
    }
    let mut env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for (key, value) in command.get_envs() {
        match value {
            Some(value) => env.insert(key.to_owned(), value.to_owned()),
            None => env.remove(key),
        };
    }
    let env = env
        .into_iter()
        .map(|(key, value)| {
            let mut entry = key.into_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            c_string(&entry)
        })
        .collect::<Result<Vec<_>>>()?;
    let argv: Vec<*mut c_char> = arguments
        .iter()
        .map(|argument| argument.as_ptr().cast_mut())
        .chain([core::ptr::null_mut()])
        .collect();
    let envp: Vec<*mut c_char> = env
        .iter()
        .map(|entry| entry.as_ptr().cast_mut())
        .chain([core::ptr::null_mut()])
        .collect();

    // Match std::process::Command: reset the signal mask and libstd's ignored SIGPIPE.
    // Unlike it, close every inherited descriptor, such as the app's proxy listeners.
    let flags = c_short::try_from(
        libc::POSIX_SPAWN_CLOEXEC_DEFAULT
            | libc::POSIX_SPAWN_SETSIGMASK
            | libc::POSIX_SPAWN_SETSIGDEF,
    )
    .context("invalid posix_spawn flags")?;
    let mut pid: libc::pid_t = 0;
    // SAFETY: each object is initialized before use and destroyed by its guard; all pointers
    // reference NUL-terminated strings and NULL-terminated arrays that outlive posix_spawn.
    unsafe {
        let mut attributes = core::ptr::null_mut();
        check(
            libc::posix_spawnattr_init(&raw mut attributes),
            "posix_spawnattr_init",
        )?;
        let mut attributes = SpawnAttributes(attributes);
        let mut actions = core::ptr::null_mut();
        check(
            libc::posix_spawn_file_actions_init(&raw mut actions),
            "posix_spawn_file_actions_init",
        )?;
        let mut actions = FileActions(actions);

        check(
            libc::posix_spawnattr_setflags(&raw mut attributes.0, flags),
            "posix_spawnattr_setflags",
        )?;
        let mut signals: libc::sigset_t = 0;
        libc::sigemptyset(&raw mut signals);
        check(
            libc::posix_spawnattr_setsigmask(&raw mut attributes.0, &raw const signals),
            "posix_spawnattr_setsigmask",
        )?;
        libc::sigaddset(&raw mut signals, libc::SIGPIPE);
        check(
            libc::posix_spawnattr_setsigdefault(&raw mut attributes.0, &raw const signals),
            "posix_spawnattr_setsigdefault",
        )?;
        check(
            set_disclaim(&raw mut attributes.0, 1),
            "responsibility_spawnattrs_setdisclaim",
        )?;
        for (fd, flags) in [
            (libc::STDIN_FILENO, libc::O_RDONLY),
            (libc::STDOUT_FILENO, libc::O_WRONLY),
            (libc::STDERR_FILENO, libc::O_WRONLY),
        ] {
            check(
                libc::posix_spawn_file_actions_addopen(
                    &raw mut actions.0,
                    fd,
                    c"/dev/null".as_ptr(),
                    flags,
                    0,
                ),
                "posix_spawn_file_actions_addopen",
            )?;
        }

        log::debug!("running command as its own responsible process: {program_name}");
        check(
            libc::posix_spawn(
                &raw mut pid,
                program.as_ptr(),
                &raw const actions.0,
                &raw const attributes.0,
                argv.as_ptr(),
                envp.as_ptr(),
            ),
            "posix_spawn",
        )
        .with_context(|| format!("failed to run {program_name}"))?;
    }

    let deadline = Instant::now() + timeout;
    loop {
        let mut status = 0;
        // SAFETY: `pid` is our unreaped child; Tokio and std reap only children they spawned.
        match unsafe { libc::waitpid(pid, &raw mut status, libc::WNOHANG) } {
            0 => {}
            -1 => {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error).context("failed to wait for command");
                }
            }
            _ => return Ok(ExitStatus::from_raw(status)),
        }
        if Instant::now() >= deadline {
            // SAFETY: as above; the blocking wait reaps the killed child.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
                libc::waitpid(pid, &raw mut status, 0);
            }
            bail!(
                "{program_name} timed out after {} seconds; the operation may be partially completed",
                timeout.as_secs()
            );
        }
        std::thread::sleep(WAIT_POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disclaimed_command_reports_exit_status() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "test \"$WIZARD_TEST\" = set && exit 3"]);
        command.env("WIZARD_TEST", "set");

        let status = command_status_disclaimed(&command, Duration::from_secs(10))
            .expect("disclaimed command runs");

        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn disclaimed_command_times_out() {
        let mut command = Command::new("/bin/sleep");
        command.arg("10");

        let error = command_status_disclaimed(&command, Duration::from_millis(200))
            .expect_err("command exceeds its timeout");

        assert!(format!("{error:#}").contains("timed out"));
    }

    #[test]
    fn detached_descendant_is_not_charged_to_this_process() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let pid_file = directory.path().join("pid");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "/bin/sleep 10 & echo $! > \"$PID_FILE\""])
            .env("PID_FILE", &pid_file);

        let status = command_status_disclaimed(&command, Duration::from_secs(10))
            .expect("disclaimed command runs");
        let descendant: libc::pid_t = std::fs::read_to_string(&pid_file)
            .expect("descendant pid")
            .trim()
            .parse()
            .expect("numeric pid");
        let own = libc::pid_t::try_from(std::process::id()).expect("pid fits");
        let ours = responsible_pid(own).expect("responsibility lookup");
        let theirs = responsible_pid(descendant).expect("responsibility lookup");
        // SAFETY: the descendant was started by this test and only sleeps.
        unsafe { libc::kill(descendant, libc::SIGKILL) };

        assert!(status.success());
        // A disclaimed shell is the descendant's responsible process; after it exits,
        // the descendant reports itself instead of inheriting ours.
        assert_ne!(theirs, ours);
    }
}
