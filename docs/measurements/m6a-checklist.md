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
| 6 | App stopped | Stop stepwave (Task Manager); CS2 goes silent; start it again and audio returns | |
| 7 | Unplug/replug output | Pull the USB headset out and back in; audio returns | |
| 8 | Missing VB-Cable message | (optional) Disable CABLE Output in `mmsys.cpl`; `status` shows the install hint; re-enable | |
| 9 | Logon start | Reboot; after logging in, `stepwave status` answers without starting it by hand | |
| 10 | Idle game audio | Pause/stay in the menu with no sound for >10 s; `stepwave status` shows no capture error note and audio resumes instantly (VB-Cable must keep delivering silence packets) | |
| 11 | Crash recovery | End only the child `stepwave.exe` (the one with `--supervised` in Task Manager's command line column) → it restarts within ~1 s; ending the parent stops both | |
| 12 | Install without admin | Run `stepwave install` from a normal (non-elevated) PowerShell; it succeeds | |
