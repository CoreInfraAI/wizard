# Read-only lookup. Exit 10 means missing; other failures must not trigger registration.
$ErrorActionPreference = 'Stop'
try {
    $scheduler = New-Object -ComObject Schedule.Service
    $scheduler.Connect()
    $folder = $scheduler.GetFolder('\')
    try {
        $null = $folder.GetTask($env:WIZARD_PROXY_TASK)
    } catch {
        # HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND), not access denied.
        if ($_.Exception.HResult -eq -2147024894 -or
            $_.Exception.InnerException.HResult -eq -2147024894) {
            exit 10
        }
        throw
    }
    exit 0
} catch {
    exit 1
}
