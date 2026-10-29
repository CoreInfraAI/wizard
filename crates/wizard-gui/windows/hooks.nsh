; Verified against the Tauri CLI 2.11.4 NSIS template.
!macro NSIS_HOOK_PREUNINSTALL
  ; Settings must survive even if Tauri's "Delete app data" box was checked.
  StrCpy $DeleteAppDataCheckboxState 0

  ClearErrors
  ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --uninstall-proxy' $0
  ${If} ${Errors}
    StrCpy $0 2
  ${EndIf}
  ${If} $0 != 0
    DetailPrint "Proxy cleanup failed. Close Wizard from the tray and retry. Settings have been preserved."
    ${IfNot} ${Silent}
    ${AndIf} $PassiveMode != 1
      MessageBox MB_ICONSTOP|MB_OK "Cannot remove the proxy task. Close Wizard from the tray and retry. No application files have been removed."
    ${EndIf}
    SetErrorLevel 2
    Abort
  ${EndIf}
!macroend
