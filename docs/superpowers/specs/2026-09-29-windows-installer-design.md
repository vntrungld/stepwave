# Windows installer — one-file setup with profiles and models

Date: 2026-09-29
Status: approved in chat

## Goal

Replace the manual copy-and-register steps of `docs/windows-setup.md` with one
`stepwave-setup-<version>-<sha>.exe`. It installs `stepwave.exe`, the profiles and the
trained models, registers the logon task and starts stepwave. It is for the owner's own
PCs only.

## Decisions (from brainstorming)

| Decision | Choice | Reason |
|---|---|---|
| Where it is built | Locally on the dev box (CachyOS), never in CI | Models are derived from game audio. They are git-ignored and must never leave the owner's machines. |
| Tool | NSIS 3 (`makensis`), natively or through Docker (`installer/Dockerfile`, ubuntu:24.04 + apt `nsis`) | NSIS is AUR-only on Arch/CachyOS. Docker needs no AUR package. NSIS cross-compiles from Linux, unlike WiX and Inno Setup. |
| `stepwave.exe` source | The `stepwave-windows` artifact of the latest green CI run of a branch (`gh run download`), or `--exe <path>` | No Windows toolchain on the dev box. CI already builds the release exe. |
| Scope | Per user: `%LOCALAPPDATA%\stepwave`, HKCU uninstall entry, `RequestExecutionLevel user` | stepwave runs as the user (per-user logon task, per-user pipe). No admin prompt. |
| VB-Cable | Detected, not bundled | Its licence forbids redistribution. Its driver needs admin rights and a reboot. |
| Publishing | Never. No GitHub Release, no CI artifact with models | Game-derived data stays private. |

## Build

`scripts/build-windows-installer.sh [--branch <b>] [--exe <path>] [--no-models] [--out <dir>]`
runs these steps:

1. Get `stepwave.exe`, either from CI (default: the current branch) or from `--exe`.
2. Stage the files in `build/installer/stage/`:
   - `stepwave.exe`
   - `profiles/*.json`
   - each model named by a profile's `model` field.

   A model that is missing gives a warning, not an error: that profile then falls back to
   its static EQ at runtime.
3. Run `makensis` with `-DSTAGE`, `-DVERSION=<Cargo version>-<sha>` and `-DOUTFILE`. It
   uses native `makensis` if present, or Docker otherwise. The paths must be absolute,
   because makensis changes into the script's directory.
4. The output is `dist/stepwave-setup-<version>-<sha>.exe`. The sha is `local` when
   `--exe` is used. Profiles must name models as `models/<name>.swm`.

`/build/` and `/dist/` are git-ignored.

## Install (`installer/stepwave.nsi`)

1. **`.onInit` — VB-Cable check.** PowerShell looks for a `Win32_SoundDevice` whose name
   contains "VB-Audio Virtual Cable".
   - If none is found, a Yes/No box offers to open vb-audio.com/Cable.
   - Setup continues either way; stepwave waits for the device.
2. **Stop any running stepwave.** It runs `taskkill /F /IM stepwave.exe`, so an upgrade
   in place can replace the exe. The supervisor's Job Object then takes the child down
   with it.
3. **Copy the files.**
   - `stepwave.exe` goes to `$INSTDIR`.
   - `profiles\*.json` and `models\*.swm` go to their subfolders; the models copy is
     `/nonfatal`, so an installer built without models still works.

   These are the paths the app already resolves: profiles from
   `%LOCALAPPDATA%\stepwave\profiles` and each model relative to the profiles' parent.
   The install folder is therefore fixed; `.onInit` ignores a `/D=` override. An upgrade
   overwrites the shipped profiles (hand edits are lost) and keeps models it no longer
   ships until uninstall.
4. **Register the uninstaller.** It writes `uninstall.exe` and the HKCU `Uninstall\stepwave`
   entry, which gives the Apps & features entry.
5. **Create the Start-menu shortcuts** in `stepwave\`:
   - **stepwave toggle**: runs `stepwave.exe toggle` minimized, with hotkey Ctrl+Alt+S.
   - **stepwave status**: a PowerShell window running `stepwave.exe status`.
   - **Windows setup guide**: an Internet shortcut (`.url`) to the guide.
   - **Uninstall stepwave**.
6. **Register the logon task** with `stepwave.exe install`. A failure is logged in the
   details; it does not abort the install.
7. **Start `stepwave.exe run` hidden** (PowerShell `Start-Process -WindowStyle Hidden`),
   so there is no console window.
8. **Show the finish page.** It gives the one manual step left: set cs2.exe → CABLE Input
   in the Volume mixer. It also covers the toggle hotkey, the FACEIT warning and a link to
   the guide.

## Uninstall

The uninstaller removes everything the install added. It refuses to run unless
`$INSTDIR\stepwave.exe` exists, and it deletes only the files it installed, never
`RMDir /r`:

1. It kills stepwave.
2. It runs `stepwave.exe uninstall`, which removes the logon task.
3. It removes the shortcuts and the HKCU key.
4. It deletes the installed files, then the now-empty folders.

## Testing

- **CI (`installer` job, ubuntu):** apt `nsis`, then run the build script with a
  placeholder exe and `--no-models`. This is a compile check of the script and the
  pipeline only. No real exe or models are involved.
- **Local:** a full build from the CI artifact includes `models/cs2.swm` and compiles
  without warnings. `--no-models` gives warning 7010 (no `*.swm` matched), which is
  expected.
- **Manual on Windows:** rows 18–20 of `docs/measurements/m6a-checklist.md` cover fresh
  install, upgrade over a running install, and uninstall.

## Out of scope

- Code signing. SmartScreen will warn; choose "More info → Run anyway".
- Automatic Volume-mixer routing (M6b).
- Silent or unattended install switches.
