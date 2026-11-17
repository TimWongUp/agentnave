; Host configuration belongs exclusively to the App. Upgrade mode is not an uninstall.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    StrCpy $R8 ""
    ClearErrors
    ${GetOptions} $CMDLINE "/AGENTNAVE_APPROVED=" $R8
    StrCpy $R9 ""
    FileOpen $R7 "$INSTDIR\.agentnave-uninstall-approved" r
    ${IfNot} ${Errors}
      FileRead $R7 $R9
      FileClose $R7
    ${EndIf}
    ${If} $R8 != ""
    ${AndIf} $R8 == $R9
      ; Consume only a matching one-shot marker. The App helper waited for App exit.
      Delete "$INSTDIR\.agentnave-uninstall-approved"
    ${Else}
      ClearErrors
      ExecWait '"$INSTDIR\${MAINBINARYNAME}.exe" --agentnave-uninstall' $R7
      ${If} ${Errors}
        SetErrorLevel 2
        Abort
      ${EndIf}
      ${If} $R7 != 0
        SetErrorLevel 1
        Abort
      ${EndIf}
    ${EndIf}
  ${EndIf}
!macroend
