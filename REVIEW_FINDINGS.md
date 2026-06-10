# Code Review Findings — Tray Fan Control + Logo LED Submenu

Review date: 2026-06-09. Scope: tray fan-control feature (commits `e1df547...HEAD`)
and the uncommitted logo LED submenu in `razer_control_gui/src/razer-settings/tray.rs`.

## Already fixed (2026-06-09, uncommitted)

- ~~**Stale `on_ac` in activate closures**~~ — closures now read AC state from
  `tray.state` at click time, and the poll loop fires `on_update()` on AC
  transitions so the menu rebuilds.
- ~~**Unbounded mpsc channel leak when `connect_activate` returns early**~~ —
  channel removed entirely; the monitor bar's 200 ms timer now reads
  `SharedSensorState` directly and re-renders only on change
  (`SensorState` derives `PartialEq`).

## Open findings

### Correctness / UX

1. ~~**Device-not-found fallback is silent**~~ — **Fixed 2026-06-09.**
   `device_info()` (typed, over `service::SupportedDevice`) distinguishes
   not-found from no-fan-data; the caller logs the device name and file path
   before falling back to defaults. Covered by the three `device_info_*` tests.

2. ~~**Daemon-down shows "Auto" as checked**~~ — **Fixed 2026-06-09.**
   `selected_preset_index` / `fan_current_label` now take `Option<i32>`;
   unknown fan speed selects no preset and the header reads "Fan Speed · …".
   Covered by `no_preset_checked_when_fan_speed_unknown` and
   `fan_label_shows_unknown_when_fan_speed_unknown`.

3. ~~**Logo header asserts "On" when state is unknown**~~ — **Fixed 2026-06-09.**
   `logo_label` falls back to "…" for `None`/out-of-range states. Covered by
   `logo_label_shows_unknown_marker_for_unknown_state`.

4. ~~**Set results ignored + optimistic checkmark**~~ — **Fixed 2026-06-09**
   (tray side). Activate closures only update local state when
   `set_acknowledged()` confirms the daemon returned `result: true`. Covered
   by `set_commands_acknowledged_only_on_true_result`.
   **Daemon side closed as by-design (2026-06-10):** returning `true` when the
   target profile is inactive is correct — the GUI's AC/Battery toggle
   legitimately edits the inactive profile (store now, apply on switch), so a
   `false` there would surface phantom errors. The tray always targets the
   active profile since the click-time AC fix.

### Performance

5. ~~**Three nvidia-smi subprocesses every 2 s**~~ — **Fixed 2026-06-09.**
   Single `read_dgpu_stats()` invocation queries temperature, power, and
   utilization together; `parse_dgpu_stats()` handles `[N/A]` fields, garbage
   output, and multi-GPU (first line). Covered by four parser tests.

6. ~~**Two socket connects per poll cycle**~~ — **Fixed 2026-06-09.**
   New `GetFanSpeedAndLogo` command (appended at enum end for bincode wire
   compatibility) returns both values in one round-trip; daemon handler added.
   Covered by bincode round-trip tests in `comms.rs` (which also guard the
   existing tray protocol variants). Note: a freshly built GUI needs the
   freshly built daemon — restart `razercontrol` after install.

### Cleanup / architecture

7. ~~**Tray re-parses laptops.json**~~ — **Fixed 2026-06-10.**
   New `GetDeviceCapabilities` IPC command: the daemon serves its own parsed
   laptops.json entry (name, features, fan), so the tray cannot drift from
   the daemon's view. Falls back to the typed GetDeviceName + local-file
   lookup for older daemons. Covered by a bincode round-trip test.

8. ~~**Duplicated protocol constants**~~ — **Fixed 2026-06-10.**
   `LOGO_LABELS` and `DEFAULT_FAN_MIN`/`DEFAULT_FAN_MAX` live in `lib.rs`;
   tray, GUI, and CLI all reference them.

9. ~~**Static device facts live inside per-poll `SensorState`**~~ —
   **Fixed 2026-06-10.** Capabilities moved into a nested `DeviceCaps` struct
   preserved as one unit by the poll loop; a newly added capability field is
   covered automatically instead of needing a per-field rescue.

### Housekeeping

10. ~~**Orphaned / stray files in working tree**~~ — **Fixed 2026-06-10.**
    `probe_bho.rs` deleted (its `[[bin]]` entry was removed in `739827e`);
    makepkg artifacts and `SESSION_REPORT.md` gitignored.

## Status

All findings resolved as of 2026-06-10. Regression coverage: 28 unit tests
(`cargo test`) spanning tray menu logic, the dgpu stats parser, device
capability lookup, set acknowledgement, and bincode round-trips for the IPC
protocol. Release versioning is guarded by the `check-version` CI job; bump
with `scripts/bump-version.sh`.

Post-review incident worth remembering: a freshly built GUI talking to a
not-yet-restarted older daemon cannot use new protocol commands — new
commands must be appended at the enum end (bincode encodes variant index)
and clients should fall back to older commands when a new one gets no
answer, as the tray now does for both of its commands.
