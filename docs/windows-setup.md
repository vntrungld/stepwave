# stepwave on Windows (M6a)

stepwave processes **only the game's audio**. CS2 plays into the free VB-Cable virtual device;
`stepwave.exe` captures it, enhances footsteps with `core`, and plays the result on your real
headset or speakers. Discord, browsers and everything else go straight to your device as usual.

> **FACEIT:** do not use stepwave in FACEIT matches until FACEIT Support has confirmed in
> writing that it is allowed. Use it in Premier/Competitive (VAC) first.

## 1. Install VB-Cable

Download "VB-CABLE Driver" from vb-audio.com, run `VBCABLE_Setup_x64.exe` as administrator,
then **reboot**. You now have a playback device **CABLE Input** and a recording device
**CABLE Output**.

## 2. Set formats to 48 kHz

Open the classic Sound Control Panel (`mmsys.cpl`):

- **Playback** tab → *CABLE Input* → Properties → Advanced → **2 channel, 24 bit, 48000 Hz**.
- **Recording** tab → *CABLE Output* → Properties → Advanced → **2 channel, 24 bit, 48000 Hz**.
- **Playback** tab → your headset/speakers → Properties → Advanced → **48000 Hz**.

stepwave asks Windows for 48 kHz float stereo and Windows converts if a device differs, but
matching formats avoids an extra conversion.

## 3. Install stepwave

Build it (`cargo build --release -p stepwave-winapp`) or download `stepwave-windows` from a CI
run, then in PowerShell:

```powershell
$d = "$env:LOCALAPPDATA\stepwave"
New-Item -ItemType Directory -Force "$d\profiles", "$d\models" | Out-Null
Copy-Item stepwave.exe $d
Copy-Item profiles\*.json "$d\profiles"
# Optional: copy trained models if you have them (models\*.swm is not in git).
# Without a model, stepwave uses the profile's static EQ.
Copy-Item models\*.swm "$d\models" -ErrorAction SilentlyContinue
& "$d\stepwave.exe" install     # start at logon (per-user task, no admin)
Start-Process "$d\stepwave.exe" -ArgumentList run -WindowStyle Hidden
& "$d\stepwave.exe" status
```

`install` registers a Task Scheduler task scoped to your own Windows account — it needs no
administrator rights, and a normal PowerShell window is enough.

At your next logon, Task Scheduler starts that task in your interactive session. `stepwave.exe`
is a console program, so a console window for it may briefly appear on screen; you can minimise
it, but **do not close it** — closing that window stops stepwave, and since CS2's output is
pinned to VB-Cable, CS2 goes silent.

`status` should show `capture: CABLE Output (VB-Audio Virtual Cable)` and your device under
`render:`.

## 4. Send CS2 to VB-Cable

Start CS2. Open **Settings → System → Sound → Volume mixer**, find **cs2.exe** and set its
**Output device** to **CABLE Input**. Windows remembers this per app.

Now `stepwave status` shows `running: model` (or `eq` if you have no model file) and the
`buffer:` line starts moving.

## 5. A/B hotkey

Create a Desktop shortcut to `"%LOCALAPPDATA%\stepwave\stepwave.exe" toggle`, open its
Properties and set a **Shortcut key** (for example Ctrl+Alt+S). Pressing it flips between
processing and bypass (same latency both ways, so the comparison is fair).

## Everyday commands

```
stepwave status                  # devices, buffer, drift, what is running
stepwave toggle                  # processing <-> bypass
stepwave mode auto|model|eq|bypass
stepwave strength 7              # dB, until reload/restart
stepwave profile cs2             # pin a profile; --auto to unpin
stepwave reload                  # re-read profiles and models
stepwave uninstall               # remove the logon task
```

## If the game goes silent

CS2 plays into VB-Cable, so if stepwave is not running you hear nothing from CS2.

`stepwave run` supervises itself: if it crashes, it restarts the app automatically (up to 3
times within a minute), so a single crash usually recovers within about a second on its own.
Check the rest only when it does not:

1. `stepwave status`: if it says stepwave is not running — either because it gave up after 3
   crashes in a minute, or because it (or its supervisor) was stopped — start it again:
   `Start-Process "$env:LOCALAPPDATA\stepwave\stepwave.exe" -ArgumentList run -WindowStyle Hidden`.
2. If `note:` says `CABLE Output not found`, reinstall VB-Cable and reboot. While the device is
   missing, stepwave keeps retrying on its own, backing off from 0.25 s up to 5 s between tries.
3. Quick escape: in the Volume mixer set cs2.exe's output back to your headset.

## Latency

Total added latency is about 50–60 ms with Windows' default 10 ms device periods: 20 ms of
processing window, about 21 ms of buffer between the two device clocks (two periods plus a
margin, see `buffer:` in `status`), and the two devices' own buffers. That is the price of
processing only the game; bypass has the same latency, so A/B comparisons are fair.
