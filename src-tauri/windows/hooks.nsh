; Installer hooks, wired up in tauri.conf.json (bundle.windows.nsis.installerHooks).
; Kept to plain ASCII: NSIS reads a script without BOM in the system code page.

; Runs before anything is copied.
;
; After an update from inside the app, the old ClippiBoy is still on its way
; out: the updater starts this installer first and only then ends the process,
; and taking down capture, audio and the GPU encoder takes a moment. The app
; check that follows can miss a process in that state, and copying over
; clippiboy.exe then fails with "Error opening file for writing".
;
; So wait here until the file can be opened for writing again - at most 20 s.
; Past that, the usual check takes over and shuts down whatever still holds it.
!macro NSIS_HOOK_PREINSTALL
  ${If} ${FileExists} "$INSTDIR\${MAINBINARYNAME}.exe"
    Push $R8
    Push $R9
    StrCpy $R9 0
    clippiboy_wait_for_exe:
      ClearErrors
      FileOpen $R8 "$INSTDIR\${MAINBINARYNAME}.exe" a
      ${IfNot} ${Errors}
        FileClose $R8
        Goto clippiboy_exe_free
      ${EndIf}
      IntOp $R9 $R9 + 1
      IntCmp $R9 80 clippiboy_exe_free
      Sleep 250
      Goto clippiboy_wait_for_exe
    clippiboy_exe_free:
    Pop $R9
    Pop $R8
  ${EndIf}
!macroend

; Runs after everything is in place.
;
; Downloads from clippiboy.com/get/<ref> are named ...-setup.<ref>.exe, so the
; installer's own file name says which link it came from. Write it down for the
; app, which reads the source out of it (telemetry.rs) - no parsing here. Only
; the first install does: an update runs the plain GitHub installer and must not
; overwrite where someone originally came from.
!macro NSIS_HOOK_POSTINSTALL
  ${IfNot} ${FileExists} "$LOCALAPPDATA\gg.clippiboy.app\install-source.txt"
    Push $R8
    CreateDirectory "$LOCALAPPDATA\gg.clippiboy.app"
    ClearErrors
    FileOpen $R8 "$LOCALAPPDATA\gg.clippiboy.app\install-source.txt" w
    ${IfNot} ${Errors}
      FileWrite $R8 "$EXEFILE"
      FileClose $R8
    ${EndIf}
    Pop $R8
  ${EndIf}
!macroend

; Runs at the end of the uninstall, after Tauri's own clean-up.
;
; Tauri's uninstaller already removes the HKCU Run entry (the autostart
; plugin names it after the product) and, with "delete app data" ticked, the
; folders named after the identifier (WebView2 data, install-source.txt). It
; does not know ClippiBoy's own data folder, %APPDATA%\ClippiBoy: settings,
; the clip library with its originals and thumbnails, logs and the sign-in
; token. With the box ticked, and never during an update, that goes too.
;
; The clip folder is the user's: if it lies inside the data folder, or the
; settings cannot be read to tell, nothing is deleted and the user is told.
; The check reads "clipDir" from config.json with plain string work, as the
; file is pretty-printed JSON with that key on a line of its own:
;   "clipDir": "C:\\Users\\me\\Videos\\ClippiBoy",
; R0: 0 = safe to delete, 1 = clip folder inside, 2 = could not tell.
!macro NSIS_HOOK_POSTUNINSTALL
  ; Windows keeps a second, per-entry on/off record for startup items.
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "${PRODUCTNAME}"
  ${EndIf}

  ${If} $DeleteAppDataCheckboxState = 1
  ${AndIf} $UpdateMode <> 1
    SetShellVarContext current
    ${If} ${FileExists} "$APPDATA\ClippiBoy\*.*"
      Push $R0
      Push $R1
      Push $R2
      Push $R3
      Push $R4
      Push $R5
      Push $R6
      Push $R7
      Push $R8
      Push $R9
      StrCpy $R0 0
      StrCpy $R4 0
      StrCpy $R5 ""

      ${If} ${FileExists} "$APPDATA\ClippiBoy\config.json"
        ClearErrors
        FileOpen $R1 "$APPDATA\ClippiBoy\config.json" r
        ${If} ${Errors}
          StrCpy $R0 2
        ${Else}
          clippiboy_cfg_line:
            ClearErrors
            FileRead $R1 $R2
            ${If} ${Errors}
              Goto clippiboy_cfg_done
            ${EndIf}
            StrCpy $R3 $R2 14
            StrCmp $R3 '  "clipDir": "' 0 clippiboy_cfg_line
            IntOp $R4 $R4 + 1
            ; Walk the value up to its closing quote: "\\" is one backslash,
            ; "/" becomes one, any other escape (or no closing quote) means
            ; the line is not what this expects.
            StrCpy $R5 ""
            StrCpy $R6 14
            clippiboy_cfg_char:
              StrCpy $R7 $R2 1 $R6
              StrCmp $R7 "" clippiboy_cfg_bad
              StrCmp $R7 '"' clippiboy_cfg_line
              StrCmp $R7 "\" 0 clippiboy_cfg_plain
                IntOp $R6 $R6 + 1
                StrCpy $R7 $R2 1 $R6
                StrCmp $R7 "\" clippiboy_cfg_plain clippiboy_cfg_bad
              clippiboy_cfg_plain:
              StrCmp $R7 "/" 0 +2
                StrCpy $R7 "\"
              StrCpy $R5 "$R5$R7"
              IntOp $R6 $R6 + 1
              Goto clippiboy_cfg_char
            clippiboy_cfg_bad:
              StrCpy $R5 ""
              StrCpy $R0 2
              Goto clippiboy_cfg_line
          clippiboy_cfg_done:
          FileClose $R1
        ${EndIf}
        ; Exactly one readable "clipDir", or the answer is "could not tell".
        ${If} $R4 <> 1
          StrCpy $R0 2
        ${EndIf}
        ${If} $R5 == ""
          StrCpy $R0 2
        ${EndIf}
      ${ElseIf} ${FileExists} "$APPDATA\ClippiBoy\config.json.bak"
        ; Only a backup left: it may hold a clip folder this cannot read.
        StrCpy $R0 2
      ${EndIf}
      ; No settings file at all: the clip folder was never changed from the
      ; default, which lies under Videos.

      ${If} $R0 = 0
      ${AndIf} $R5 != ""
        ; Inside: the value is the data folder itself or lies below it. The
        ; second test is for a user name this script cannot read right
        ; (the file is UTF-8, the script reads it as ANSI).
        StrCpy $R8 "$APPDATA\ClippiBoy"
        StrLen $R9 $R8
        StrCpy $R3 $R5 $R9
        ${If} $R3 == $R8
          StrCpy $R3 $R5 1 $R9
          ${If} $R3 == ""
          ${OrIf} $R3 == "\"
            StrCpy $R0 1
          ${EndIf}
        ${EndIf}
        StrCpy $R6 0
        clippiboy_in_scan:
          ${If} $R0 <> 0
            Goto clippiboy_in_done
          ${EndIf}
          StrCpy $R7 $R5 1 $R6
          StrCmp $R7 "" clippiboy_in_done
          StrCpy $R3 $R5 26 $R6
          ${If} $R3 == "\AppData\Roaming\ClippiBoy"
            IntOp $R9 $R6 + 26
            StrCpy $R3 $R5 1 $R9
            ${If} $R3 == ""
            ${OrIf} $R3 == "\"
              StrCpy $R0 1
            ${EndIf}
          ${EndIf}
          IntOp $R6 $R6 + 1
          Goto clippiboy_in_scan
        clippiboy_in_done:
      ${EndIf}

      ${If} $R0 = 0
        RmDir /r "$APPDATA\ClippiBoy"
      ${ElseIf} $R0 = 1
        MessageBox MB_OK|MB_ICONINFORMATION "ClippiBoy's data folder was not deleted because your clip folder lies inside it:$\r$\n$\r$\n$R5$\r$\n$\r$\nMove your clips somewhere else first, then delete this folder yourself:$\r$\n$APPDATA\ClippiBoy" /SD IDOK
      ${Else}
        MessageBox MB_OK|MB_ICONINFORMATION "ClippiBoy's data folder was not deleted because its settings could not be read to check where your clips are kept. Your clips are untouched.$\r$\n$\r$\nTo remove the remaining data, delete this folder yourself once your clips are safe:$\r$\n$APPDATA\ClippiBoy" /SD IDOK
      ${EndIf}

      Pop $R9
      Pop $R8
      Pop $R7
      Pop $R6
      Pop $R5
      Pop $R4
      Pop $R3
      Pop $R2
      Pop $R1
      Pop $R0
    ${EndIf}
  ${EndIf}
!macroend
