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

!macro NSIS_HOOK_PREINSTALL
  Push "$INSTDIR\${MAINBINARYNAME}.exe"
  Call GlowdriftRequestAppQuit
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  Push "$INSTDIR\${MAINBINARYNAME}.exe"
  Call un.GlowdriftRequestAppQuit
!macroend

; Removes the vendor folder above the install directory once nothing else is in it. RMDir without /r
; never removes a folder that still has content, and the folder name must equal the manufacturer, so
; a custom location given with /D is never touched.
!macro NSIS_HOOK_POSTUNINSTALL
  ${un.GetParent} "$INSTDIR" $0
  ${un.GetFileName} "$0" $1
  ${If} $1 == "${MANUFACTURER}"
    RMDir "$0"
  ${EndIf}
!macroend
