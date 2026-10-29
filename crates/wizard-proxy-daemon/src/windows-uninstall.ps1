# Invoked by Rust with -NoProfile -NonInteractive. Values travel through the
# child environment, never through PowerShell source interpolation.
$ErrorActionPreference = 'Stop'
try {
    $scheduler = New-Object -ComObject Schedule.Service
    $scheduler.Connect()
    $folder = $scheduler.GetFolder('\')
    try {
        $task = $folder.GetTask($env:WIZARD_UNINSTALL_TASK)
    } catch {
        # HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND), not access denied.
        if ($_.Exception.HResult -eq -2147024894 -or
            $_.Exception.InnerException.HResult -eq -2147024894) {
            exit 10
        }
        throw
    }

    $actions = $task.Definition.Actions
    if ($actions.Count -ne 1) { throw 'Unexpected task actions' }
    $action = $actions.Item(1)
    if ($action.Type -ne 0 -or $action.Arguments -cne '--proxy' -or
        -not [string]::Equals(
            [IO.Path]::GetFullPath($action.Path),
            [IO.Path]::GetFullPath($env:WIZARD_UNINSTALL_EXE),
            [StringComparison]::OrdinalIgnoreCase)) {
        throw 'Task belongs to another installation'
    }

    switch ($env:WIZARD_UNINSTALL_OPERATION) {
        'disable' { $task.Enabled = $false }
        'delete' {
            # No forced termination: Rust already waited for graceful shutdown.
            if ($task.GetInstances(0).Count -ne 0) {
                throw 'Proxy task still has running instances; retry uninstall'
            }
            $folder.DeleteTask($env:WIZARD_UNINSTALL_TASK, 0)
        }
        default { throw 'Invalid cleanup operation' }
    }
    exit 0
} catch {
    # Do not emit task XML, settings, or environment values.
    [Console]::Error.WriteLine('Task Scheduler cleanup failed.')
    exit 1
}
