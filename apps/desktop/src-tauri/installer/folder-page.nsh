; Install folder page for the first interactive install. Included by installer.nsi after the product
; defines (MANUFACTURER, PRODUCTNAME, UNINSTKEY, ESTIMATEDSIZE, PLACEHOLDER_INSTALL_DIR) and before the
; pages. The template must also have PassiveMode and UpdateMode variables, and nsDialogs, LogicLib and
; Win\COM.nsh available.
;
; The page is skipped for silent, passive and update runs, when /D= was given, and when an install is
; already registered. Both flags are computed once in FolderPageDetect, called from .onInit, because the
; reinstall page can run the previous uninstaller before this page is reached.
;
; Validation never opens a message box. nsDialogs has no notification for an edit field losing keyboard
; focus (it routes only EN_CHANGE and button clicks), so a short timer polls GetFocus and validates when
; focus moves away from the field. The result is shown in a label under the field. Next re-validates and
; stays on the page while the path is invalid; Back and Cancel are never blocked.
;
; Every function that nsDialogs can call back, and every helper, preserves $0-$9. Helpers pass arguments
; and results through the global variables $FpA, $FpB and $FpR.

!define FP_MAX_PATH_LENGTH 200
!define FP_CLSID_FileOpenDialog {DC1C5A9C-E88A-4dde-A5A1-60F82A20AEF7}
!define FP_IID_IFileDialog {42F85136-DB7E-439C-85F1-E4075D135FC8}

Var FpDirGiven
Var FpExisting
Var FpPageActive
Var FpBusy
Var FpHadFocus
Var FpEdit
Var FpBrowse
Var FpSpace
Var FpErr
Var FpPath
Var FpMsg
Var FpSpaceText
Var FpBlocked
Var FpA
Var FpB
Var FpR

; Message texts. Invoked once per installer language, right after that language is inserted.
!macro GlowdriftFolderPageLangStrings
  LangString fpHeaderTitle 0 "Choose install folder"
  LangString fpHeaderSubtitle 0 "Choose the folder in which to install ${PRODUCTNAME}."
  LangString fpIntro 0 "Setup will install ${PRODUCTNAME} in the folder below. To use a different folder, type a path or click Browse."
  LangString fpBrowse 0 "Browse..."
  LangString fpPickerTitle 0 "Choose a folder for ${PRODUCTNAME}"
  LangString fpSpaceRequired 0 "Space required:"
  LangString fpSpaceAvailable 0 "Space available:"
  LangString fpErrEmpty 0 "Enter the folder to install into."
  LangString fpErrNotAbsolute 0 "Enter a full path on a local drive, for example C:\Tools\${PRODUCTNAME}."
  LangString fpErrTooLong 0 "The path is too long. Use at most ${FP_MAX_PATH_LENGTH} characters."
  LangString fpErrEnvironment 0 "The path cannot contain environment variables such as %NAME%."
  LangString fpErrCharacters 0 "The path contains characters that Windows does not allow in folder names."
  LangString fpErrNoDrive 0 "That drive does not exist."
  LangString fpErrNotLocal 0 "Choose a folder on a local drive. Network drives and optical drives are not supported."
  LangString fpErrDriveRoot 0 "Choose a folder, not a whole drive."
  LangString fpErrProtected 0 "Choose a folder that does not need administrator rights. The Program Files and Windows folders are not allowed."
  LangString fpErrIsFile 0 "Part of this path is a file, not a folder. Choose another folder."
  LangString fpErrNotWritable 0 "Setup cannot write to this folder. Choose another one."
  LangString fpErrNoSpace 0 "There is not enough free space on this drive."
!macroend

; Called from .onInit after SetContext and before $INSTDIR is defaulted: a $INSTDIR that already differs
; from the placeholder was set by /D=.
Function FolderPageDetect
  Push $0
  StrCpy $FpDirGiven 0
  ${If} $INSTDIR != "${PLACEHOLDER_INSTALL_DIR}"
    StrCpy $FpDirGiven 1
  ${EndIf}
  StrCpy $FpExisting 0
  ReadRegStr $0 SHCTX "${UNINSTKEY}" "UninstallString"
  ${If} $0 != ""
    StrCpy $FpExisting 1
  ${EndIf}
  Pop $0
FunctionEnd

; In: $FpA path. Out: $FpR parent folder, or "" when the path is a drive root or has no parent.
Function FpParentDir
  Push $0
  Push $1
  Push $2
  StrCpy $FpR ""
  StrLen $0 $FpA
  ${If} $0 > 3
    StrCpy $1 $0
    ${Do}
      IntOp $1 $1 - 1
      ${If} $1 <= 0
        ${ExitDo}
      ${EndIf}
      StrCpy $2 $FpA 1 $1
      ${If} $2 == "\"
        StrCpy $FpR $FpA $1
        ${If} $1 == 2
          StrCpy $FpR "$FpR\"
        ${EndIf}
        ${ExitDo}
      ${EndIf}
    ${Loop}
  ${EndIf}
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; In: $FpA path, $FpB base. Out: $FpR 1 when the path equals the base or lies below it.
Function FpIsUnder
  Push $0
  Push $1
  StrCpy $FpR 0
  ${If} $FpB != ""
    StrLen $0 $FpB
    StrCpy $1 $FpA $0
    ${If} $1 == $FpB
      StrCpy $1 $FpA 1 $0
      ${If} $1 == ""
      ${OrIf} $1 == "\"
        StrCpy $FpR 1
      ${EndIf}
    ${EndIf}
  ${EndIf}
  Pop $1
  Pop $0
FunctionEnd

; In: $FpA path. Out: $FpR the path itself or its nearest ancestor that is an existing folder, or ""
; when none is found. $FpBlocked is 1 when the nearest existing entry is a file. Changes $FpA.
Function FpNearestExisting
  Push $0
  Push $1
  Push $2
  StrCpy $FpBlocked 0
  StrCpy $0 $FpA
  StrCpy $2 0
  ${Do}
    IntOp $2 $2 + 1
    ${If} $2 > 64
      StrCpy $0 ""
      ${ExitDo}
    ${EndIf}
    System::Call 'kernel32::GetFileAttributesW(w r0)i .r1'
    ${If} $1 != -1
    ${AndIf} $1 != 4294967295
      IntOp $1 $1 & 0x10
      ${If} $1 == 0
        StrCpy $FpBlocked 1
        StrCpy $0 ""
      ${EndIf}
      ${ExitDo}
    ${EndIf}
    StrCpy $FpA $0
    Call FpParentDir
    StrCpy $0 $FpR
    ${If} $0 == ""
      ${ExitDo}
    ${EndIf}
  ${Loop}
  StrCpy $FpR $0
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; In: $FpA folder (not a drive root). Out: $FpR 1 when it exists and has no entries.
Function FpFolderIsEmpty
  Push $0
  Push $1
  StrCpy $FpR 0
  FindFirst $0 $1 "$FpA\*.*"
  ${If} $0 != ""
    StrCpy $FpR 1
    ${Do}
      ${If} $1 == ""
        ${ExitDo}
      ${EndIf}
      ${If} $1 != "."
      ${AndIf} $1 != ".."
        StrCpy $FpR 0
        ${ExitDo}
      ${EndIf}
      FindNext $0 $1
    ${Loop}
    FindClose $0
  ${EndIf}
  Pop $1
  Pop $0
FunctionEnd

!macro FpFailIfProtected BASE
  StrCpy $FpA $0
  StrCpy $FpB "${BASE}"
  Call FpIsUnder
  ${If} $FpR == 1
    StrCpy $FpMsg "$(fpErrProtected)"
    Goto validate_done
  ${EndIf}
!macroend

; Validates the text in the path field. Out: $FpPath the normalized path, $FpMsg the message for the
; first rule that failed or "" when the path is acceptable. Updates the labels under the field.
Function FpValidate
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7
  Push $8
  Push $9

  StrCpy $FpMsg ""
  StrCpy $6 0
  IntOp $7 ${ESTIMATEDSIZE} / 1024
  IntOp $7 $7 + 1
  StrCpy $FpSpaceText "$(fpSpaceRequired) $7 MB"

  ; Normalize: surrounding spaces and trailing backslashes (except the one of a drive root) are dropped.
  ${NSD_GetText} $FpEdit $0
  ${Do}
    StrCpy $1 $0 1
    ${If} $1 != " "
      ${ExitDo}
    ${EndIf}
    StrCpy $0 $0 "" 1
  ${Loop}
  ${Do}
    StrCpy $1 $0 1 -1
    StrLen $2 $0
    ${If} $1 == " "
      StrCpy $0 $0 -1
    ${ElseIf} $1 == "\"
    ${AndIf} $2 > 3
      StrCpy $0 $0 -1
    ${Else}
      ${ExitDo}
    ${EndIf}
  ${Loop}
  StrCpy $FpPath $0

  ${If} $0 == ""
    StrCpy $FpMsg "$(fpErrEmpty)"
    Goto validate_done
  ${EndIf}
  StrLen $1 $0
  ${If} $1 > ${FP_MAX_PATH_LENGTH}
    StrCpy $FpMsg "$(fpErrTooLong)"
    Goto validate_done
  ${EndIf}

  ; Absolute path on a drive letter: X:\...
  StrCpy $1 $0 1 1
  StrCpy $2 $0 1 2
  ${If} $1 != ":"
  ${OrIf} $2 != "\"
    StrCpy $FpMsg "$(fpErrNotAbsolute)"
    Goto validate_done
  ${EndIf}

  ; Characters after "X:\"
  StrCpy $3 3
  StrCpy $5 "\"
  ${Do}
    StrCpy $4 $0 1 $3
    ${If} $4 == ""
      ${ExitDo}
    ${EndIf}
    ${If} $4 == "%"
      StrCpy $FpMsg "$(fpErrEnvironment)"
      Goto validate_done
    ${EndIf}
    ${If} $4 == "<"
    ${OrIf} $4 == ">"
    ${OrIf} $4 == ":"
    ${OrIf} $4 == "$\""
    ${OrIf} $4 == "/"
    ${OrIf} $4 == "|"
    ${OrIf} $4 == "?"
    ${OrIf} $4 == "*"
      StrCpy $FpMsg "$(fpErrCharacters)"
      Goto validate_done
    ${EndIf}
    ${If} $4 == "\"
      ${If} $5 == "\"
        StrCpy $FpMsg "$(fpErrCharacters)"
        Goto validate_done
      ${EndIf}
    ${EndIf}
    StrCpy $5 $4
    IntOp $3 $3 + 1
  ${Loop}
  ${If} $5 == "."
    StrCpy $FpMsg "$(fpErrCharacters)"
    Goto validate_done
  ${EndIf}

  ; Fixed or removable local drive that exists
  StrCpy $1 $0 3
  System::Call 'kernel32::GetDriveTypeW(w r1)i .r2'
  ${If} $2 <= 1
    StrCpy $FpMsg "$(fpErrNoDrive)"
    Goto validate_done
  ${EndIf}
  ${If} $2 == 4
  ${OrIf} $2 == 5
    StrCpy $FpMsg "$(fpErrNotLocal)"
    Goto validate_done
  ${EndIf}
  StrLen $2 $0
  ${If} $2 == 3
    StrCpy $FpMsg "$(fpErrDriveRoot)"
    Goto validate_done
  ${EndIf}

  ; Free space of the drive; the verdict is given last so the other rules report first.
  StrCpy $2 ""
  System::Call 'kernel32::GetDiskFreeSpaceExW(w, *l, *l, *l)i(r1,.r2,.r3,.)'
  ${If} $2 != ""
    System::Int64Op $2 / 1048576
    Pop $2
    StrCpy $FpSpaceText "$FpSpaceText$\r$\n$(fpSpaceAvailable) $2 MB"
    ${If} $2 < $7
      StrCpy $6 1
    ${EndIf}
  ${EndIf}

  ; Folders that need elevation to write
  !insertmacro FpFailIfProtected $PROGRAMFILES
  !insertmacro FpFailIfProtected $PROGRAMFILES64
  !insertmacro FpFailIfProtected $WINDIR

  ; Writable without elevation: probe the folder itself, or the nearest existing ancestor when the folder
  ; does not exist yet and Setup will create it.
  StrCpy $FpA $0
  Call FpNearestExisting
  StrCpy $5 $FpR
  ${If} $5 == ""
    ${If} $FpBlocked == 1
      StrCpy $FpMsg "$(fpErrIsFile)"
    ${Else}
      StrCpy $FpMsg "$(fpErrNotWritable)"
    ${EndIf}
    Goto validate_done
  ${EndIf}
  System::Call 'kernel32::GetTickCount()i .r1'
  StrCpy $2 $5 1 -1
  ${If} $2 == "\"
    StrCpy $8 "$5.write-test-$1"
  ${Else}
    StrCpy $8 "$5\.write-test-$1"
  ${EndIf}
  ${If} $5 == $0
    ; 0x40010000 = GENERIC_WRITE | DELETE, 1 = CREATE_NEW, 0x04000000 = FILE_FLAG_DELETE_ON_CLOSE
    System::Call 'kernel32::CreateFileW(w r8, i 0x40010000, i 0, p 0, i 1, i 0x04000000, p 0)i .r9'
    ${If} $9 == -1
    ${OrIf} $9 == 0
      StrCpy $FpMsg "$(fpErrNotWritable)"
      Goto validate_done
    ${EndIf}
    System::Call 'kernel32::CloseHandle(i r9)'
  ${Else}
    System::Call 'kernel32::CreateDirectoryW(w r8, p 0)i .r9'
    ${If} $9 == 0
      StrCpy $FpMsg "$(fpErrNotWritable)"
      Goto validate_done
    ${EndIf}
    System::Call 'kernel32::RemoveDirectoryW(w r8)'
  ${EndIf}

  ${If} $6 == 1
    StrCpy $FpMsg "$(fpErrNoSpace)"
  ${EndIf}

  validate_done:
  ${NSD_SetText} $FpSpace $FpSpaceText
  ; The error label is transparent, so it is hidden while its text changes to avoid overdrawn text.
  ShowWindow $FpErr ${SW_HIDE}
  ${NSD_SetText} $FpErr $FpMsg
  ${If} $FpMsg != ""
    ShowWindow $FpErr ${SW_SHOW}
  ${EndIf}

  Pop $9
  Pop $8
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; Edit field changed: the shown message may be stale, so hide it. The next focus loss, Browse or Next
; validates again. nsDialogs puts the control handle on the stack.
Function FpOnChange
  Exch $0
  ShowWindow $FpErr ${SW_HIDE}
  Pop $0
FunctionEnd

; Timer: validate once when keyboard focus leaves the path field. Ignored while the folder picker is
; open, because the picker takes focus.
Function FpOnTimer
  Push $0
  Push $1
  ${If} $FpPageActive == 1
  ${AndIf} $FpBusy != 1
    System::Call 'user32::IsWindow(p $FpEdit)i .r0'
    ${If} $0 <> 0
      System::Call 'user32::GetFocus()p .r1'
      ${If} $1 == $FpEdit
        StrCpy $FpHadFocus 1
      ${ElseIf} $FpHadFocus == 1
        StrCpy $FpHadFocus 0
        Call FpValidate
      ${EndIf}
    ${EndIf}
  ${EndIf}
  Pop $1
  Pop $0
FunctionEnd

Function FpOnBack
  StrCpy $FpPageActive 0
  ${NSD_KillTimer} FpOnTimer
FunctionEnd

; Browse: IFileDialog (CLSID_FileOpenDialog) as a folder picker. Vtable slots: Show 3, SetOptions 9,
; GetOptions 10, SetFolder 12, SetTitle 17, GetResult 20; IShellItem::GetDisplayName 5; Release 2.
; Any failure leaves the field unchanged.
Function FpOnBrowse
  Exch $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7
  Push $8
  Push $9

  ${If} $FpBusy != 1
    StrCpy $FpBusy 1
    StrCpy $2 0
    StrCpy $8 ""
    !insertmacro ComHlpr_CreateInProcInstance ${FP_CLSID_FileOpenDialog} ${FP_IID_IFileDialog} r2 ""
    ${If} $2 != 0
    ${AndIf} $2 != ""
      ; FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST, added to the default options
      System::Call "$2->10(*i .r3)i .r9"
      StrCmp $9 0 0 browse_release
      IntOp $3 $3 | 0x860
      System::Call "$2->9(i r3)i .r9"
      StrCmp $9 0 0 browse_release
      StrCpy $4 "$(fpPickerTitle)"
      System::Call "$2->17(w r4)i"

      ; Start in the nearest existing folder of the current text
      ${NSD_GetText} $FpEdit $5
      StrCpy $FpA $5
      Call FpNearestExisting
      StrCpy $5 $FpR
      ${If} $5 != ""
        StrCpy $6 0
        System::Call 'SHELL32::SHCreateItemFromParsingName(w r5, p 0, g"${IID_IShellItem}", *p .r6)i .r9'
        ${If} $6 != 0
        ${AndIf} $6 != ""
          System::Call "$2->12(p r6)i"
          System::Call "$6->2()i"
        ${EndIf}
      ${EndIf}

      System::Call "$2->3(p $HWNDPARENT)i .r9"
      StrCmp $9 0 0 browse_release
      StrCpy $6 0
      System::Call "$2->20(*p .r6)i .r9"
      StrCmp $9 0 0 browse_release
      ${If} $6 != 0
      ${AndIf} $6 != ""
        StrCpy $7 0
        ; SIGDN_FILESYSPATH is 0x80000000. The copy is bounded so a very long path cannot overrun the
        ; buffer; a truncated path is longer than the allowed maximum and fails validation.
        System::Call "$6->5(i -2147483648, *p .r7)i .r9"
        ${If} $9 = 0
        ${AndIf} $7 != 0
          System::Call 'kernel32::lstrcpynW(w .r8, p r7, i 400)'
          System::Call 'OLE32::CoTaskMemFree(p r7)'
        ${EndIf}
        System::Call "$6->2()i"
      ${EndIf}

      browse_release:
      System::Call "$2->2()i"
    ${EndIf}

    ${If} $8 != ""
      ; The picked folder is a parent. It is used as is when it is already named like the product or is
      ; an empty folder; otherwise the product folder is added. A drive root always gets the product folder.
      StrLen $4 $8
      ${If} $4 <= 3
        StrCpy $8 $8 3
        StrCpy $5 $8 1 -1
        ${If} $5 != "\"
          StrCpy $8 "$8\"
        ${EndIf}
        StrCpy $8 "$8${PRODUCTNAME}"
      ${Else}
        StrCpy $5 $8 1 -1
        ${If} $5 == "\"
          StrCpy $8 $8 -1
        ${EndIf}
        StrCpy $FpA $8
        Call FpParentDir
        StrLen $5 $FpR
        StrCpy $6 $8 "" $5
        StrCpy $7 $6 1
        ${If} $7 == "\"
          StrCpy $6 $6 "" 1
        ${EndIf}
        ${If} $6 != "${PRODUCTNAME}"
          StrCpy $FpA $8
          Call FpFolderIsEmpty
          ${If} $FpR != 1
            StrCpy $8 "$8\${PRODUCTNAME}"
          ${EndIf}
        ${EndIf}
      ${EndIf}
      ${NSD_SetText} $FpEdit $8
      Call FpValidate
    ${EndIf}
    StrCpy $FpBusy 0
  ${EndIf}

  Pop $9
  Pop $8
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

Function FolderPageCreate
  ${If} ${Silent}
  ${OrIf} $PassiveMode = 1
  ${OrIf} $UpdateMode = 1
  ${OrIf} $FpDirGiven = 1
  ${OrIf} $FpExisting = 1
    Abort
  ${EndIf}

  !insertmacro MUI_HEADER_TEXT "$(fpHeaderTitle)" "$(fpHeaderSubtitle)"
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ${IfThen} $(^RTL) = 1 ${|} nsDialogs::SetRTL $(^RTL) ${|}

  ${NSD_CreateLabel} 0 0 100% 24u "$(fpIntro)"
  Pop $0

  ${NSD_CreateText} 0 28u -56u 12u "$INSTDIR"
  Pop $FpEdit

  ${NSD_CreateButton} -50u 27u 50u 14u "$(fpBrowse)"
  Pop $FpBrowse

  ${NSD_CreateLabel} 0 48u 100% 22u ""
  Pop $FpSpace

  ${NSD_CreateLabel} 0 74u 100% 24u ""
  Pop $FpErr
  SetCtlColors $FpErr 0xCC0000 transparent

  StrCpy $FpBusy 0
  StrCpy $FpHadFocus 0
  StrCpy $FpPageActive 1
  Call FpValidate

  ${NSD_SetFocus} $FpEdit
  ${NSD_OnChange} $FpEdit FpOnChange
  ${NSD_OnClick} $FpBrowse FpOnBrowse
  ${NSD_OnBack} FpOnBack
  ${NSD_CreateTimer} FpOnTimer 200

  nsDialogs::Show
FunctionEnd

Function FolderPageLeave
  Call FpValidate
  ${If} $FpMsg != ""
    ${NSD_SetFocus} $FpEdit
    Abort
  ${EndIf}
  StrCpy $FpPageActive 0
  ${NSD_KillTimer} FpOnTimer
  StrCpy $INSTDIR $FpPath
FunctionEnd
