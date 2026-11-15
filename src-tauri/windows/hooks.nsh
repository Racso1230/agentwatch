; NSIS installer hooks, wired in by src-tauri/tauri.release.conf.json
; (bundle.windows.nsis.installerHooks).
;
; On uninstall, remove agentwatch's entries from Claude Code's settings.json
; so Claude Code is not left calling a deleted binary.
;
; Upgrades must NOT do this. Findings from Tauri's NSIS template
; (crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi, "dev" branch,
; https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/nsis/installer.nsi):
;  - When a new installer finds an old install, it runs the OLD uninstaller
;    with "_?=<dir>", and appends "/UPDATE" when the new installer itself was
;    started in update mode (the Tauri updater does this; see "reinst_uninstall").
;  - The uninstaller reads that flag in un.onInit into $UpdateMode (1 = update).
;  - NSIS_HOOK_PREUNINSTALL is expanded at the very top of "Section Uninstall",
;    before the "is the app running?" check, so $UpdateMode is already set.
; So we skip the disconnect when $UpdateMode = 1.
;
; Caveat: if a user starts a newer installer by hand and picks "Uninstall
; before installing" on the reinstall page, that is a normal uninstall
; ($UpdateMode = 0), so hooks are removed; the app's Connect button restores them.
;
; The hook runs before the template's CheckIfAppIsRunning, and
; `agentwatch.exe --disconnect-hooks` exits immediately without a window, so it
; works whether or not the app is open. The uninstaller itself still needs the
; app closed: it uses the Windows Restart Manager to offer to close it.
!macro NSIS_HOOK_PREUNINSTALL
  ${If} $UpdateMode <> 1
    ExecWait '"$INSTDIR\agentwatch.exe" --disconnect-hooks'
  ${EndIf}
!macroend
