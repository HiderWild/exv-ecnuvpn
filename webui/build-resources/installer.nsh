; installer.nsh — NSIS custom installer script for EXV Desktop
;
; This script is included by electron-builder during NSIS packaging.
; It handles optional service registration on install and clean service
; teardown on uninstall.
; It does not install the global exv CLI command; that is managed from
; the desktop Settings page.
;
; Behavior summary:
;   INSTALL:
;     - After files are copied, offers to install the exv-helper Windows
;       service (recommended, not forced). UAC is already active during
;       NSIS install (perMachine=true), so no additional elevation needed.
;     - The service install runs: bin\exv.exe service install
;     - If the user declines, the app still works — each VPN session will
;       trigger a one-time UAC elevation prompt instead.
;
;   UNINSTALL:
;     - Before files are removed, stops and unregisters the exv-helper
;       service if it exists: bin\exv.exe service uninstall
;     - This prevents a dangling service pointing to deleted files.
;

!macro customInstall
  ; Offer to install the privileged helper service.
  ; The NSIS installer already runs elevated (perMachine=true), so
  ; exv.exe service install will succeed without a second UAC prompt.
  MessageBox MB_YESNO \
    "Install the ECNU VPN helper service?$\n$\n\
    The service allows VPN connections without repeated admin prompts.$\n\
    You can install it later from the app's Service page.$\n$\n\
    Install service now?" \
    /SD IDYES IDYES installService IDNO skipService

installService:
  DetailPrint "Installing exv-helper service..."
  nsExec::ExecToLog '"$INSTDIR\bin\exv.exe" service install'
  Pop $0
  ${If} $0 == 0
    DetailPrint "Service installed successfully."
  ${Else}
    DetailPrint "Service install returned exit code $0 (non-fatal)."
    DetailPrint "You can install the service later from the app."
  ${EndIf}
  Goto doneService

skipService:
  DetailPrint "Skipped service installation. You can install it later from the app."

doneService:
!macroend

!macro customUnInstall
  ; Unregister the helper service before files are deleted.
  ; Check if exv.exe still exists (user may have deleted it manually).
  ${If} ${FileExists} "$INSTDIR\bin\exv.exe"
    DetailPrint "Uninstalling exv-helper service..."
    nsExec::ExecToLog '"$INSTDIR\bin\exv.exe" service uninstall'
    Pop $0
    ${If} $0 == 0
      DetailPrint "Service uninstalled successfully."
    ${Else}
      DetailPrint "Service uninstall returned exit code $0 (non-fatal)."
      DetailPrint "The service may have already been removed."
    ${EndIf}
  ${Else}
    DetailPrint "exv.exe not found — skipping service uninstall."
  ${EndIf}
!macroend

!macro customInstallMode
  ; Force per-machine install (already set by perMachine=true in
  ; package.json, but this makes it explicit).
  StrCpy $IsInstallMode "AllUsers"
!macroend
