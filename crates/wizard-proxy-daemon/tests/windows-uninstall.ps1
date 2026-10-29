# Run on Windows: powershell -NoProfile -ExecutionPolicy Bypass -File <this file>
# Only creates/deletes a uniquely named test task; never touches Wizard settings.
$ErrorActionPreference = 'Stop'
$script = [IO.File]::ReadAllText((Join-Path $PSScriptRoot '../src/windows-uninstall.ps1'))
$scheduler = New-Object -ComObject Schedule.Service
$scheduler.Connect()
$folder = $scheduler.GetFolder('\')
$name = 'wizard-uninstall-test-' + [Guid]::NewGuid().ToString('N')
$env:WIZARD_UNINSTALL_TASK = $name
$env:WIZARD_UNINSTALL_EXE = "$env:SystemRoot\System32\cmd.exe"

function Invoke-Cleanup($operation, $expected) {
    $env:WIZARD_UNINSTALL_OPERATION = $operation
    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -Command $script
    if ($LASTEXITCODE -ne $expected) { throw "Unexpected exit code for $operation : $LASTEXITCODE" }
}

try {
    Invoke-Cleanup disable 10
    Invoke-Cleanup delete 10

    $definition = $scheduler.NewTask(0)
    $definition.Principal.UserId = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $definition.Principal.LogonType = 3
    $definition.Settings.ExecutionTimeLimit = 'PT1M'
    $action = $definition.Actions.Create(0)
    $action.Path = $env:WIZARD_UNINSTALL_EXE
    $action.Arguments = '--proxy'
    $task = $folder.RegisterTaskDefinition($name, $definition, 6, $null, $null, 3)

    $env:WIZARD_UNINSTALL_EXE = "$env:SystemRoot\System32\notepad.exe"
    Invoke-Cleanup disable 1
    Invoke-Cleanup delete 1
    if (-not $folder.GetTask($name).Enabled) { throw 'Foreign task was disabled' }

    $env:WIZARD_UNINSTALL_EXE = $action.Path
    Invoke-Cleanup disable 0
    if ($folder.GetTask($name).Enabled) { throw 'Task was not disabled' }
    Invoke-Cleanup disable 0
    Invoke-Cleanup delete 0
    Invoke-Cleanup delete 10

    # Wrong arguments must not be treated as our proxy, even for the same EXE.
    $action.Arguments = '/c exit 0'
    $task = $folder.RegisterTaskDefinition($name, $definition, 6, $null, $null, 3)
    Invoke-Cleanup disable 1
    Invoke-Cleanup delete 1
    Write-Host 'Windows uninstall task tests passed.'
} finally {
    try { $folder.DeleteTask($name, 0) } catch {
        if ($_.Exception.HResult -ne -2147024894 -and
            $_.Exception.InnerException.HResult -ne -2147024894) { throw }
    }
    Remove-Item Env:WIZARD_UNINSTALL_TASK, Env:WIZARD_UNINSTALL_EXE, Env:WIZARD_UNINSTALL_OPERATION -ErrorAction SilentlyContinue
}
