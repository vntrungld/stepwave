; stepwave per-user installer (NSIS 3). Built by scripts/build-windows-installer.sh.
; Expects STAGE (a directory with stepwave.exe, profiles\*.json and optionally models\*.swm),
; VERSION and OUTFILE to be passed with -D.

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"

!ifndef STAGE
  !error "pass -DSTAGE=<dir>"
!endif
!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef OUTFILE
  !define OUTFILE "stepwave-setup.exe"
!endif

!define APP "stepwave"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\stepwave"
!define GUIDE_URL "https://github.com/vntrungld/stepwave/blob/main/docs/windows-setup.md"
!define VBCABLE_URL "https://vb-audio.com/Cable/"

Name "${APP} ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\${APP}"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ShowInstDetails show

!define MUI_FINISHPAGE_TITLE "stepwave is installed and running"
!define MUI_FINISHPAGE_TEXT "One step is left, in Windows:$\r$\n$\r$\nStart CS2, open Settings > System > Sound > Volume mixer, and set cs2.exe's Output device to CABLE Input.$\r$\n$\r$\nToggle processing with Ctrl+Alt+S (Start menu > stepwave > stepwave toggle). Check it with 'stepwave status'.$\r$\n$\r$\nFACEIT: do not use stepwave in FACEIT matches until FACEIT Support confirms in writing that it is allowed."
!define MUI_FINISHPAGE_LINK "Open the Windows setup guide"
!define MUI_FINISHPAGE_LINK_LOCATION "${GUIDE_URL}"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; Exit code 0 when VB-Cable's "VB-Audio Virtual Cable" sound device is present.
!define VBCABLE_CHECK `powershell.exe -NoProfile -NonInteractive -Command "if (Get-CimInstance Win32_SoundDevice | Where-Object { $$_.Name -like '*VB-Audio Virtual Cable*' }) { exit 0 } else { exit 1 }"`

Function .onInit
  ; stepwave reads its profiles from %LOCALAPPDATA%\stepwave\profiles, so the install
  ; folder is fixed (a /D= override would install where the app never looks).
  StrCpy $INSTDIR "$LOCALAPPDATA\${APP}"
  nsExec::ExecToStack `${VBCABLE_CHECK}`
  Pop $0
  Pop $1
  ; $0 is the exit code, or "error" if PowerShell could not be started.
  ${If} $0 S!= "0"
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "VB-Cable was not found.$\r$\n$\r$\nstepwave needs the free VB-Cable virtual device (the game plays into it). Install 'VB-CABLE Driver' from vb-audio.com as administrator and reboot.$\r$\n$\r$\nOpen the download page now? (Setup continues either way; stepwave waits until VB-Cable appears.)" IDNO +2
    ExecShell "open" "${VBCABLE_URL}"
  ${EndIf}
FunctionEnd

Section "stepwave" SecMain
  ; Upgrade in place: stop the supervisor and the app if they are running.
  nsExec::Exec 'taskkill.exe /F /IM stepwave.exe'
  Pop $0
  Sleep 500

  SetOutPath "$INSTDIR"
  File "${STAGE}\stepwave.exe"
  SetOutPath "$INSTDIR\profiles"
  File "${STAGE}\profiles\*.json"
  SetOutPath "$INSTDIR\models"
  File /nonfatal "${STAGE}\models\*.swm"
  SetOutPath "$INSTDIR"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "stepwave"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\stepwave.exe"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1

  CreateDirectory "$SMPROGRAMS\${APP}"
  CreateShortCut "$SMPROGRAMS\${APP}\stepwave toggle.lnk" "$INSTDIR\stepwave.exe" "toggle" "$INSTDIR\stepwave.exe" 0 SW_SHOWMINIMIZED CONTROL|ALT|S "Switch between processing and bypass"
  CreateShortCut "$SMPROGRAMS\${APP}\stepwave status.lnk" "powershell.exe" `-NoExit -NoProfile -Command "& '$INSTDIR\stepwave.exe' status"` "$INSTDIR\stepwave.exe" 0 SW_SHOWNORMAL "" "Show what stepwave is doing"
  WriteINIStr "$SMPROGRAMS\${APP}\Windows setup guide.url" "InternetShortcut" "URL" "${GUIDE_URL}"
  CreateShortCut "$SMPROGRAMS\${APP}\Uninstall stepwave.lnk" "$INSTDIR\uninstall.exe"

  ; Start at logon (per-user Task Scheduler task), then start now, hidden.
  nsExec::ExecToLog '"$INSTDIR\stepwave.exe" install'
  Pop $0
  ${If} $0 != 0
    DetailPrint "stepwave install returned $0; start stepwave manually with 'stepwave run'"
  ${EndIf}
  Exec `powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -Command "Start-Process -FilePath '$INSTDIR\stepwave.exe' -ArgumentList run -WindowStyle Hidden"`
SectionEnd

Section "Uninstall"
  ; Only ever delete a folder that holds stepwave (guards against /D=<some other dir>).
  ${IfNot} ${FileExists} "$INSTDIR\stepwave.exe"
    MessageBox MB_OK|MB_ICONSTOP "stepwave.exe was not found in $INSTDIR; nothing was removed."
    Abort
  ${EndIf}
  nsExec::Exec 'taskkill.exe /F /IM stepwave.exe'
  Pop $0
  Sleep 500
  nsExec::ExecToLog '"$INSTDIR\stepwave.exe" uninstall'
  Pop $0
  Delete "$SMPROGRAMS\${APP}\*.lnk"
  Delete "$SMPROGRAMS\${APP}\*.url"
  RMDir "$SMPROGRAMS\${APP}"
  DeleteRegKey HKCU "${UNINST_KEY}"
  Delete "$INSTDIR\stepwave.exe"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$INSTDIR\profiles\*.json"
  Delete "$INSTDIR\models\*.swm"
  RMDir "$INSTDIR\profiles"
  RMDir "$INSTDIR\models"
  RMDir "$INSTDIR"
SectionEnd
