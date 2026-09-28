# M5 manual checklist (Linux daemon)

Run on the dev box with the daemon installed as a user service
(`systemctl --user status stepwave` is active). Tick each item and note anything odd.

| # | Check | How | Result |
|---|---|---|---|
| 1 | CS2 is routed automatically | Start CS2; `stepwave status` shows `routed: cs2`, `running: model` | |
| 2 | Other apps are untouched | Play a browser video and join voice chat; `pw-link -l` shows them on the device, not on `stepwave` | |
| 3 | `toggle` makes no click | Bind `stepwave toggle` to a KDE global shortcut; toggle repeatedly during footsteps | |
| 4 | Strength and mode apply live | `stepwave strength 4`, `stepwave mode eq`, `stepwave mode auto` while playing | |
| 5 | Output device switch | Switch the default output (e.g. speakers → headphones) mid-game; audio follows | |
| 6 | PipeWire restart recovers | `systemctl --user restart pipewire wireplumber` mid-game; within ~5 s the game is routed again | |
| 7 | Daemon crash is harmless | `systemctl --user kill -s KILL stepwave`; game audio continues on the device; systemd restarts the daemon | |
| 8 | Manual move is respected | Move the game stream to the device in pavucontrol; the daemon does not pull it back until the game restarts | |
| 9 | Latency feels unchanged | Play a round; no perceptible delay between action and sound | |
| 10 | No feedback loop | Select `stepwave` as the default output in KDE; selecting stepwave as the default output does not create a loop (stepwave-output stays on the real device, `pw-link -l`); switch back afterwards | |

## Notes

- Latency (automated, `output_delay_is_the_reported_latency` in
  `daemon/tests/pipewire.rs`): with a quantum of 256 the recorder shows 1216 samples
  between the dry and processed click, of which one quantum is the recorder's own async
  link, so stepwave adds exactly 960 samples (20 ms). Before the playback stream was
  triggered from the capture callback it added 1216 (960 + one quantum).
