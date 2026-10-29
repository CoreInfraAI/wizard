# Windows uninstall

The NSIS pre-uninstall hook invokes the installed executable with
`--uninstall-proxy` before Tauri checks running processes or deletes files.
Exit Wizard from the tray first: a shared GUI lock prevents cleanup while any
current GUI instance is running. Cleanup holds the exclusive lock until it exits.

Cleanup validates the current user's task action against the installed EXE,
disables the task, requests HTTP shutdown and waits for process exit, then deletes
the task. It never force-kills the daemon. A refused connection is not proof of
exit: cleanup also acquires the daemon instance lock and checks task instances.
Without a registered task, an occupied instance lock causes cleanup to fail
rather than sending shutdown to an unidentified server.

Windows PowerShell (the OS-provided WindowsPowerShell executable) accesses Task
Scheduler through COM. Task names and paths are passed in the child environment,
not interpolated into script source. No additional package is required. Policy
restrictions on PowerShell/COM cause uninstall to fail safely.

Errors abort before file removal, also in silent/passive mode. A failure after
disabling the task leaves it disabled; retry uninstall after resolving the error.
Starting Wizard again normally restores registration when no daemon is listening.
If the EXE is missing, repair/reinstall before retrying uninstall.

Settings and logs are preserved. The hook clears Tauri 2.11.4's
`DeleteAppDataCheckboxState`, even if its stock checkbox was selected.
The GUI/uninstall coordination file `proxy-uninstall.lock` is intentionally retained
alongside the existing daemon lock; unlinking live lock files is unsafe.

## Verification

Windows `cargo x ci` includes a test using a uniquely named temporary scheduler
task. It verifies missing-task handling, repeated cleanup, disable/delete, and
rejection of a different executable or arguments. It does not touch the real
Wizard task or settings.

Before release, also test the **built NSIS installer**:
- Running proxy, closed GUI: uninstall stops the process and removes its task.
- Open GUI: uninstall aborts without removing files; exit via tray and retry.
- Proxy starting or unresponsive: cleanup either waits or aborts without force-kill.
- Already absent task and stopped proxy: uninstall succeeds.
- Normal, silent (`/S`), and passive (`/P`) uninstall preserve `config.toml`,
  including when the stock delete-data checkbox is selected.
- Upgrade/reinstall: settings survive and the next GUI start registers the task.
- Paths with spaces and non-ASCII characters.

Recheck the hook position, `MAINBINARYNAME`, `PassiveMode`, and
`DeleteAppDataCheckboxState` when updating Tauri's NSIS template.
