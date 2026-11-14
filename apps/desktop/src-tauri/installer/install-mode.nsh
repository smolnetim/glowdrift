; Install mode handling for installMode "both" (all users or current user). Included by installer.nsi
; after MultiUser.nsh, and only for that mode. MultiUser.nsh provides the mode page, the two mode switches
; (MultiUser.InstallMode.AllUsers and MultiUser.InstallMode.CurrentUser) and $MultiUser.InstallMode. This
; file replaces MULTIUSER_INIT and MULTIUSER_UNINIT, because MultiUser.nsh can only ask Windows for
; elevation at launch (RequestExecutionLevel highest), which prompts for a per-user install as well.
;
; The setup file starts without elevation. The mode is decided in .onInit:
;   1. An existing installation decides: the Uninstall key in HKLM is all users, the one in HKCU is current
;      user. Updates and reinstalls therefore never change the mode.
;   2. Otherwise /allusers or /currentuser, the last one given wins.
;   3. Otherwise all users.
; A switch for the other mode than the existing installation is refused, so two installations never exist.
;
; The mode page is shown only when nothing above decided the mode: an interactive first install without a
; switch. Elevation is requested only for all users, when the process is not elevated yet: at launch when
; the mode is decided, or when the mode page is left with all users selected. The setup file then starts
; itself again with "runas", waits for it and returns its exit code. The new process gets the /ELEVATED
; flag, which makes it skip the pages that were already shown and never elevates again. The uninstaller
; does the same when it belongs to an all users installation.
;
; The installer is a 32-bit program, which the SHELLEXECUTEINFO layout in ImRelaunch relies on.

Var ImElevated
Var ImElevatedChild
Var ImRequested
Var ImAskMode
Var ImParams
Var ImExit
Var ImMessage

; Message texts. Invoked once per installer language, right after that language is inserted.
!macro GlowdriftInstallModeLangStrings
  LangString imErrInstalledAllUsers 0 "${PRODUCTNAME} is already installed for all users of this computer. Uninstall it before installing it for the current user only."
  LangString imErrInstalledCurrentUser 0 "${PRODUCTNAME} is already installed for the current user only. Uninstall it before installing it for all users."
  LangString imErrNoElevation 0 "Setup needs administrator rights to work for all users, and Windows did not grant them. To install without them, choose the option for the current user only."
!macroend

; Functions that exist in the installer and in the uninstaller.
!macro GlowdriftInstallModeShared PREFIX

  ; Out: $ImElevated 1 when the process token is elevated, otherwise 0. TokenElevation is information
  ; class 20 and TOKEN_QUERY is 8.
  Function ${PREFIX}ImDetectElevation
    Push $0
    Push $1
    Push $2
    Push $3
    Push $4
    StrCpy $ImElevated 0
    System::Call 'kernel32::GetCurrentProcess()p .r0'
    System::Call 'advapi32::OpenProcessToken(p r0, i 8, *p .r1)i .r2'
    ${If} $2 <> 0
      System::Call 'advapi32::GetTokenInformation(p r1, i 20, *i .r2, i 4, *i .r3)i .r4'
      ${If} $4 <> 0
      ${AndIf} $2 <> 0
        StrCpy $ImElevated 1
      ${EndIf}
      System::Call 'kernel32::CloseHandle(p r1)'
    ${EndIf}
    Pop $4
    Pop $3
    Pop $2
    Pop $1
    Pop $0
  FunctionEnd

  ; In: $ImParams the parameters for the new process. Out: $ImExit the exit code of the new process, or
  ; "" when it could not be started, for example because the prompt was declined. 0x140 is
  ; SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC, the fields after nShow are unused, and hProcess is the last
  ; of the 15 fields.
  Function ${PREFIX}ImRelaunch
    Push $0
    Push $1
    Push $2
    Push $3
    StrCpy $ImExit ""
    System::Call '*(i 60, i 0x140, p 0, t "runas", t "$EXEPATH", t "$ImParams", t "$EXEDIR", i 1, p 0, p 0, p 0, p 0, p 0, p 0, p 0)p .r0'
    System::Call 'shell32::ShellExecuteExW(p r0)i .r1'
    ${If} $1 <> 0
      System::Call '*$0(i, i, i, i, i, i, i, i, i, i, i, i, i, i, p .r2)'
      System::Call 'kernel32::WaitForSingleObject(p r2, i -1)i'
      System::Call 'kernel32::GetExitCodeProcess(p r2, *i .r3)i'
      System::Call 'kernel32::CloseHandle(p r2)'
      StrCpy $ImExit $3
    ${EndIf}
    System::Free $0
    Pop $3
    Pop $2
    Pop $1
    Pop $0
  FunctionEnd

  ; In: $ImMessage. Prints to the console of a silent run, otherwise shows a message box.
  Function ${PREFIX}ImReport
    Push $0
    ${If} ${Silent}
      System::Call 'kernel32::AttachConsole(i -1)i.r0'
      ${If} $0 <> 0
        System::Call 'kernel32::GetStdHandle(i -11)i.r0'
        System::Call 'kernel32::SetConsoleTextAttribute(i r0, i 0x0004)'
        FileWrite $0 "$ImMessage$\r$\n"
      ${EndIf}
    ${Else}
      MessageBox MB_ICONSTOP "$ImMessage"
    ${EndIf}
    Pop $0
  FunctionEnd

  ; In: $ImParams. Runs this program again with administrator rights and ends it with the exit code of the
  ; new process. When the new process cannot be started the message is reported and the program ends.
  Function ${PREFIX}ImRelaunchAndQuit
    Call ${PREFIX}ImRelaunch
    ${If} $ImExit == ""
      StrCpy $ImMessage "$(imErrNoElevation)"
      Call ${PREFIX}ImReport
      SetErrorLevel 1223
    ${Else}
      SetErrorLevel $ImExit
    ${EndIf}
    Quit
  FunctionEnd
!macroend
!insertmacro GlowdriftInstallModeShared ""
!insertmacro GlowdriftInstallModeShared "un."

; Installer: decides the mode, applies it (shell folders, registry hive, default folder) and elevates when
; needed. Called from .onInit after FolderPageDetect, which is why /D= is available in $FpDirGiven.
Function ImInitInstallMode
  Push $0
  Push $1
  Push $2
  Push $3

  Call ImDetectElevation
  ; Any account may choose all users: Windows asks for administrator credentials when it is not one.
  StrCpy $MultiUser.Privileges "Admin"

  StrCpy $ImElevatedChild 0
  ${GetOptions} $CMDLINE "/ELEVATED" $0
  ${IfNot} ${Errors}
    StrCpy $ImElevatedChild 1
  ${EndIf}

  StrCpy $ImRequested ""
  ${GetOptions} $CMDLINE "/currentuser" $0
  ${IfNot} ${Errors}
    StrCpy $ImRequested "CurrentUser"
  ${EndIf}
  ${GetOptions} $CMDLINE "/allusers" $0
  ${IfNot} ${Errors}
    StrCpy $ImRequested "AllUsers"
  ${EndIf}

  ; $1 and $2 are not empty when installed for all users and for the current user
  ReadRegStr $1 HKLM "${UNINSTKEY}" "UninstallString"
  ReadRegStr $2 HKCU "${UNINSTKEY}" "UninstallString"

  ${If} $ImRequested == "AllUsers"
    StrCpy $3 "AllUsers"
    ${If} $1 == ""
    ${AndIf} $2 != ""
      StrCpy $ImMessage "$(imErrInstalledCurrentUser)"
      Call ImReport
      Abort
    ${EndIf}
  ${ElseIf} $ImRequested == "CurrentUser"
    StrCpy $3 "CurrentUser"
    ${If} $2 == ""
    ${AndIf} $1 != ""
      StrCpy $ImMessage "$(imErrInstalledAllUsers)"
      Call ImReport
      Abort
    ${EndIf}
  ${ElseIf} $1 == ""
  ${AndIf} $2 != ""
    StrCpy $3 "CurrentUser"
  ${Else}
    StrCpy $3 "AllUsers"
  ${EndIf}

  ; The mode page is for the one case where nothing decided the mode
  StrCpy $ImAskMode 0
  ${If} $ImRequested == ""
  ${AndIf} $1 == ""
  ${AndIf} $2 == ""
  ${AndIfNot} ${Silent}
  ${AndIf} $PassiveMode <> 1
  ${AndIf} $UpdateMode <> 1
  ${AndIf} $ImElevatedChild = 0
    StrCpy $ImAskMode 1
  ${EndIf}

  ${If} $3 == "AllUsers"
    Call MultiUser.InstallMode.AllUsers
  ${Else}
    Call MultiUser.InstallMode.CurrentUser
  ${EndIf}

  ${If} $3 == "AllUsers"
  ${AndIf} $ImElevated = 0
  ${AndIf} $ImElevatedChild = 0
  ${AndIf} $ImAskMode = 0
    Call ImRelaunchWithAllUsers
  ${EndIf}

  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; Out: $ImParams the parameters for the elevated copy of the setup file: the original parameters, with
; /D= added again last because it is not part of $CMDLINE.
Function ImBuildAllUsersParams
  Push $0
  ${GetParameters} $0
  StrCpy $ImParams "/allusers /ELEVATED $0"
  ${If} $FpDirGiven = 1
    StrCpy $ImParams "$ImParams /D=$FpGivenDir"
  ${EndIf}
  Pop $0
FunctionEnd

; Starts the setup file again with administrator rights for all users and ends this process.
Function ImRelaunchWithAllUsers
  Call ImBuildAllUsersParams
  Call ImRelaunchAndQuit
FunctionEnd

; Mode page, before it is shown: only when ImInitInstallMode left the choice open.
Function ImModePagePre
  ${If} $ImAskMode <> 1
    Abort
  ${EndIf}
FunctionEnd

; Mode page, after the selection was applied: all users needs elevation. When it is declined the page
; stays, so the user can choose the current user instead.
Function ImModePageLeave
  ${If} $MultiUser.InstallMode == "AllUsers"
  ${AndIf} $ImElevated = 0
    HideWindow
    Call ImBuildAllUsersParams
    Call ImRelaunch
    ${If} $ImExit == ""
      BringToFront
      MessageBox MB_ICONEXCLAMATION "$(imErrNoElevation)"
      Abort
    ${EndIf}
    SetErrorLevel $ImExit
    Quit
  ${EndIf}
FunctionEnd

; Uninstaller: the mode is the one of the installation this uninstaller belongs to, found by the
; uninstall command that each hive holds. An all users installation needs elevation.
Function un.ImInitInstallMode
  Push $0
  Push $1
  Push $2
  Push $3

  Call un.ImDetectElevation
  StrCpy $MultiUser.Privileges "Admin"

  StrCpy $ImElevatedChild 0
  ${GetOptions} $CMDLINE "/ELEVATED" $0
  ${IfNot} ${Errors}
    StrCpy $ImElevatedChild 1
  ${EndIf}

  ReadRegStr $1 HKLM "${UNINSTKEY}" "UninstallString"
  ReadRegStr $2 HKCU "${UNINSTKEY}" "UninstallString"
  StrCpy $3 "$\"$INSTDIR\uninstall.exe$\""
  ${If} $1 == $3
    StrCpy $0 "AllUsers"
  ${ElseIf} $2 == $3
    StrCpy $0 "CurrentUser"
  ${ElseIf} $1 != ""
    StrCpy $0 "AllUsers"
  ${Else}
    StrCpy $0 "CurrentUser"
  ${EndIf}

  ${If} $0 == "AllUsers"
    Call un.MultiUser.InstallMode.AllUsers
    ${If} $ImElevated = 0
    ${AndIf} $ImElevatedChild = 0
      ${GetParameters} $1
      StrCpy $ImParams "/ELEVATED $1"
      Call un.ImRelaunchAndQuit
    ${EndIf}
  ${Else}
    Call un.MultiUser.InstallMode.CurrentUser
  ${EndIf}

  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; Page skip for pages the elevated process already showed in the process that started it.
Function SkipIfPassiveOrElevatedChild
  ${IfThen} $PassiveMode = 1 ${|} Abort ${|}
  ${IfThen} $ImElevatedChild = 1 ${|} Abort ${|}
FunctionEnd

; Sets the shell folder context of the uninstaller back to the mode of the installation.
Function un.ImApplyShellContext
  ${If} $MultiUser.InstallMode == "AllUsers"
    SetShellVarContext all
  ${Else}
    SetShellVarContext current
  ${EndIf}
FunctionEnd
