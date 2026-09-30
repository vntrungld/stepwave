# M6a manual checklist (Windows app)

Run on the Windows PC after following `docs/windows-setup.md`. Premier/Competitive only until
FACEIT confirms. Tick each item and note anything odd.

| # | Check | How | Result |
|---|---|---|---|
| 1 | CS2 is processed, nothing else | `stepwave status` shows `running: model` (or `eq`); Discord and a browser video play normally and are not affected by `toggle` | |
| 2 | `toggle` makes no click | Toggle repeatedly with the hotkey during footsteps | |
| 3 | Headset/speaker switch | Change the default output device mid-game; audio follows within ~1 s | |
| 4 | 30 minutes, no drift | After 30 min of play: `resyncs 0`, `buffer:` near its target, no growing delay | |
| 5 | Perceived latency | Shoot/jump and listen: acceptable for play? Note it | |
| 6 | App stopped | End the parent `stepwave.exe` (without `--supervised`): both stop and CS2 goes silent; start stepwave again and audio returns | |
| 7 | Unplug/replug output | Pull the USB headset out and back in; audio returns | |
| 8 | Missing VB-Cable message | (optional) Disable CABLE Output in `mmsys.cpl`; `status` shows the install hint; re-enable | |
| 9 | Logon start | Reboot; after logging in, `stepwave status` answers without starting it by hand | |
| 10 | Idle game audio | Pause/stay in the menu with no sound for >10 s; `stepwave status` shows no capture error note and audio resumes instantly (VB-Cable must keep delivering silence packets) | |
| 11 | Crash recovery | End only the child `stepwave.exe` (the one with `--supervised` in Task Manager's command line column) → it restarts within ~1 s; ending the parent stops both | |
| 12 | Install without admin | Run `stepwave install` from a normal (non-elevated) PowerShell; it succeeds | |
| 13 | Default device guard | Temporarily set CABLE Input as the default playback device: `status` shows the note, no feedback; set the headset back: audio returns within ~1 s | |
| 14 | Sleep not blocked | With stepwave running and no game, run `powercfg /requests` (admin PowerShell); note whether stepwave/audio appears under SYSTEM or DISPLAY | |
| 15 | Sleep/resume | Sleep the PC with stepwave running, wake it; within ~3 s `stepwave status` shows both devices open and audio works | |
| 16 | Slip crossfade | During long footstep-heavy play listen for any periodic tick/flutter (drift corrections) | |
| 17 | Measured latency | Record mic + game output with a loopback recorder (e.g. play a click in CS2 console vs. headset output) and note the measured delay | |
| 18 | Installer: fresh install | Run `stepwave-setup-*.exe` as a normal user on a PC without stepwave: no admin prompt, Start-menu shortcuts appear, `stepwave status` answers, Ctrl+Alt+S toggles | |
| 19 | Installer: upgrade | Run a newer installer while stepwave is running: it stops the old one, replaces files, starts again; profiles/models present | |
| 20 | Installer: uninstall | Apps & features → stepwave → Uninstall: process stopped, logon task gone (`schtasks /Query /TN stepwave` fails), shortcuts and `%LOCALAPPDATA%\stepwave` removed | |
