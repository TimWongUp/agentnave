; Host configuration belongs exclusively to the App. Upgrades are not uninstalls: the updater
; passes /UPDATE, and a setup run by hand runs the old uninstaller in place (_?=) from $INSTDIR,
; while an ordinary uninstall runs from a temporary copy.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
  ${AndIf} $EXEDIR != $INSTDIR
    StrCpy $R8 ""
    ClearErrors
    ${GetOptions} $CMDLINE "/AGENTNAVE_APPROVED=" $R8
    StrCpy $R9 ""
    ClearErrors
    FileOpen $R7 "$INSTDIR\.agentnave-uninstall-approved" r
    ${IfNot} ${Errors}
      FileRead $R7 $R9
      FileClose $R7
    ${EndIf}
    ; One-shot: a stale marker must neither approve a later run nor keep $INSTDIR non-empty.
    Delete "$INSTDIR\.agentnave-uninstall-approved"
    ${If} $R8 != ""
    ${AndIf} $R8 == $R9
      ; Approved by the App, whose helper waited for it to exit.
    ${ElseIfNot} ${Silent}
      ; An unattended /S uninstall cannot answer the App, so it only removes program files.
      ClearErrors
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --agentnave-uninstall' $R7
      ${If} ${Errors}
        SetErrorLevel 2
        Abort
      ${EndIf}
      ${If} $R7 != 0
        SetErrorLevel $R7
        Abort
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend
