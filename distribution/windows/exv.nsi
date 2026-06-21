Unicode true
ManifestDPIAware true
RequestExecutionLevel user

!ifndef APP_VERSION
  !error "APP_VERSION define is required"
!endif

!ifndef SOURCE_DIR
  !error "SOURCE_DIR define is required"
!endif

!ifndef OUTPUT_FILE
  !error "OUTPUT_FILE define is required"
!endif

!ifndef UNINSTALL_MANIFEST
  !error "UNINSTALL_MANIFEST define is required"
!endif

!ifndef DEFAULT_INSTALL_DIR
  !define DEFAULT_INSTALL_DIR "$LOCALAPPDATA\Programs\EXV"
!endif

!define APP_NAME "EXV"
!define COMPANY_NAME "EXV"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\EXV"

Name "${APP_NAME}"
Caption "${APP_NAME} Setup"
OutFile "${OUTPUT_FILE}"
InstallDir "${DEFAULT_INSTALL_DIR}"
InstallDirRegKey HKCU "Software\EXV" "InstallDir"
Icon "..\..\assets\icons\icon.ico"
UninstallIcon "..\..\assets\icons\icon.ico"

Page directory
Page instfiles
UninstPage uninstConfirm
UninstPage instfiles

Section "Install"
  SetShellVarContext current
  SetOutPath "$INSTDIR"
  File /r "${SOURCE_DIR}\*.*"

  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\EXV" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${APP_VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "${COMPANY_NAME}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\exv-ui.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\Uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\EXV"
  CreateShortCut "$SMPROGRAMS\EXV\EXV.lnk" "$INSTDIR\exv-ui.exe" "" "$INSTDIR\exv-ui.exe"
  CreateShortCut "$SMPROGRAMS\EXV\Uninstall EXV.lnk" "$INSTDIR\Uninstall.exe" "" "$INSTDIR\Uninstall.exe"
SectionEnd

Section "Uninstall"
  SetShellVarContext current
  Delete "$SMPROGRAMS\EXV\EXV.lnk"
  Delete "$SMPROGRAMS\EXV\Uninstall EXV.lnk"
  RMDir "$SMPROGRAMS\EXV"

  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegValue HKCU "Software\EXV" "InstallDir"
  DeleteRegKey /ifempty HKCU "Software\EXV"

  !include "${UNINSTALL_MANIFEST}"
SectionEnd
