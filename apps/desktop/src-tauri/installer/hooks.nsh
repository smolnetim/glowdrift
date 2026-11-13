; Hooks for the NSIS installer (bundle.windows.nsis.installerHooks). The installer template includes this
; file before it defines MAINBINARYNAME and MANUFACTURER, so those are only referenced inside the
; NSIS_HOOK_* macros, which expand inside the install and uninstall sections.

; How long to wait for a running instance to exit after asking it to quit, in half-second steps.
!define GLOWDRIFT_QUIT_WAIT_STEPS 20

; Asks a running instance to quit through its --quit flag, so the engine stops and lighting control is
; released before files are replaced, and waits for it to exit. The wait is bounded: if the instance
; is still running afterwards, or the flag is ignored, the template's own running-app check continues
; as it would without this hook.
;
; Stack in: path of the main executable. An executable that is running cannot be opened for appending,
; which is how a running instance is detected. A missing file is skipped so that nothing is created.
!macro GlowdriftRequestAppQuitFunction PREFIX
  Function ${PREFIX}GlowdriftRequestAppQuit
    Exch $0
    Push $1
    Push $2

    IfFileExists $0 0 quit_done
    ClearErrors
    FileOpen $1 $0 a
    ${IfNot} ${Errors}
      FileClose $1
      Goto quit_done
    ${EndIf}

    Exec '"$0" --quit'
    StrCpy $2 0
    quit_wait:
      Sleep 500
      ClearErrors
      FileOpen $1 $0 a
      ${IfNot} ${Errors}
        FileClose $1
        Goto quit_done
      ${EndIf}
      IntOp $2 $2 + 1
      ${If} $2 < ${GLOWDRIFT_QUIT_WAIT_STEPS}
        Goto quit_wait
      ${EndIf}

    quit_done:
    Pop $2
    Pop $1
    Pop $0
  FunctionEnd
!macroend
!insertmacro GlowdriftRequestAppQuitFunction ""
!insertmacro GlowdriftRequestAppQuitFunction "un."

; Where the app registers its autostart entry: a value named after the product, holding the path of the
; main executable followed by its arguments, quoted or not.
!define GLOWDRIFT_RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

; Rebuilds an autostart command so that it starts the executable at a new path with the same arguments.
; The executable ends at the first ".exe" (any letter case) that is followed by the end of the value, a
; space, a tab or a closing quote, so folders with spaces need no quoting rules. What follows the
; executable, including its leading space, is kept as it was.
;
; Stack in (top first): the new executable path, the old command.
; Stack out: the new command, quoted as "path" followed by the original arguments, or an empty string
; when the old command is empty or contains no executable.
Function GlowdriftRebuildRunValue
  Exch $0
  Exch
  Exch $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7

  StrLen $2 $1
  StrCpy $3 0
  StrCpy $6 0
  StrCpy $4 $1 1
  ${If} $4 == "$\""
    StrCpy $6 1
    StrCpy $3 1
  ${EndIf}

  rebuild_scan:
    IntOp $5 $3 + 4
    ${If} $5 > $2
      StrCpy $0 ""
      Goto rebuild_done
    ${EndIf}
    StrCpy $4 $1 4 $3
    ${If} $4 == ".exe"
      ${If} $5 == $2
        Goto rebuild_found
      ${EndIf}
      StrCpy $7 $1 1 $5
      ${If} $7 == " "
      ${OrIf} $7 == "$\t"
      ${OrIf} $7 == "$\""
        Goto rebuild_found
      ${EndIf}
    ${EndIf}
    IntOp $3 $3 + 1
    Goto rebuild_scan

  rebuild_found:
    StrCpy $4 $1 "" $5
    ${If} $6 = 1
      StrCpy $7 $4 1
      ${If} $7 == "$\""
        StrCpy $4 $4 "" 1
      ${EndIf}
    ${EndIf}
    StrCpy $0 "$\"$0$\"$4"

  rebuild_done:
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Exch $0
FunctionEnd

!macro NSIS_HOOK_PREINSTALL
  Push "$INSTDIR\${MAINBINARYNAME}.exe"
  Call GlowdriftRequestAppQuit
!macroend

; Points an existing autostart entry at the executable in the install folder, so it keeps working
; after the install folder changes. Nothing is written when there is no entry, or when its value has no
; executable in it.
!macro NSIS_HOOK_POSTINSTALL
  ReadRegStr $0 HKCU "${GLOWDRIFT_RUN_KEY}" "${PRODUCTNAME}"
  ${If} $0 != ""
    Push $0
    Push "$INSTDIR\${MAINBINARYNAME}.exe"
    Call GlowdriftRebuildRunValue
    Pop $0
    ${If} $0 != ""
      WriteRegStr HKCU "${GLOWDRIFT_RUN_KEY}" "${PRODUCTNAME}" $0
    ${EndIf}
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Push "$INSTDIR\${MAINBINARYNAME}.exe"
  Call un.GlowdriftRequestAppQuit
!macroend

; Forgets the remembered install folder on every uninstall except the one an update runs, so a later
; first install starts from the default folder again. The vendor key goes only when nothing else is in it.
;
; Removes the vendor folder above the install directory once nothing else is in it. RMDir without /r
; never removes a folder that still has content, and the folder name must equal the manufacturer, so
; a custom location given with /D is never touched.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $UpdateMode <> 1
    DeleteRegKey SHCTX "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty SHCTX "${MANUKEY}"
  ${EndIf}

  ${un.GetParent} "$INSTDIR" $0
  ${un.GetFileName} "$0" $1
  ${If} $1 == "${MANUFACTURER}"
    RMDir "$0"
  ${EndIf}
!macroend
