# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Install

All Rust work happens inside `razer_control_gui/`:

```bash
# Build all three binaries
cd razer_control_gui && cargo build --release

# Install locally (binaries + udev rules + systemd service)
./install-local.sh

# Install/uninstall via script (from razer_control_gui/)
./install.sh install
./install.sh uninstall

# Install KDE Plasma widget
cd razer_control_gui/kde-widget && ./install-plasmoid.sh
```

**Rust edition 2024** — requires nightly. There is no automated test suite; `cargo build --release` is the primary validation. Manual testing is documented in `razer_control_gui/TESTING_CHECKLIST.md`.

## Architecture

Three Rust binaries communicate over a Unix socket:

```
┌──────────────┐  ┌──────────────┐  ┌────────────────┐
│  razer-cli   │  │razer-settings│  │  KDE widget    │
│  (CLI, clap) │  │ (GTK4+libadw)│  │(Plasma 6, C++) │
└──────┬───────┘  └──────┬───────┘  └──────┬─────────┘
       │ bincode         │ bincode         │ JSON
       └────────┬────────┘                 │
                ▼                          ▼
        ┌──────────────────────────────────────┐
        │              daemon                  │
        │  Unix socket: $XDG_RUNTIME_DIR/      │
        │  razercontrol-socket                 │
        │  HID API → Razer hardware            │
        └──────────────────────────────────────┘
```

- `src/daemon/` — Privileged daemon: HID hardware control, config persistence, D-Bus integration (UPower, Mutter, login1)
- `src/razer-settings/` — GTK4 + libadwaita GUI; no direct hardware access
- `src/cli/` — CLI via clap; no direct hardware access
- `src/comms.rs` — Shared IPC: `DaemonCommand`/`DaemonResponse` enums, bincode serialization
- `src/lib.rs` — Shared `SupportedDevice` struct
- `kde-widget/` — Separate C++/Qt6/KDE Frameworks 6 Plasma applet using JSON over `~/.local/share/razer-daemon.sock`

## Key Conventions

**IPC protocol**: Every new hardware feature needs a matching variant in both `DaemonCommand` and `DaemonResponse` in `comms.rs`, handled in `daemon.rs`'s socket loop, and exposed via CLI in `cli.rs` and/or GUI in `razer-settings.rs`.

**Power profiles**: `Configuration.power[]` is indexed `[0] = AC`, `[1] = Battery`. Power modes: `0=Balanced, 1=Gaming, 2=Creator, 3=Silent, 4=Custom`. Config persists to `~/.local/share/razercontrol/daemon.json`.

**Device features**: Devices declare capabilities in `laptops.json` via `features[]` array. Valid tags: `"fan"`, `"logo"`, `"boost"`, `"per_key_rgb"`, `"bho"`, `"creator_mode"`. Always check `device.has_feature("...")` before sending hardware commands.

**Error handling**: Use `Result<T, E>` throughout. Avoid `unwrap()` in daemon code paths. GUI uses `crash_with_msg()` and `setup_panic_hook()` from `error_handling.rs`.

## Adding a New Device

1. Add entry to `razer_control_gui/data/devices/laptops.json`:
   ```json
   {"name": "Blade XX 20XX", "vid": "1532", "pid": "XXXX", "features": [...], "fan": [min, max]}
   ```
2. Add PID to `razer_control_gui/data/udev/99-hidraw-permissions.rules`
3. Run `cargo build --release` and test on hardware

## Daemon Service

Runs as a systemd **user** service (no root):

```bash
systemctl --user status razercontrol
systemctl --user restart razercontrol
journalctl --user -u razercontrol -f
```

## Packaging

RPM (`packaging/fedora/razercontrol.spec`), DEB, and tarball are built via GitHub Actions on `v*` tag push. NixOS support via `flake.nix` at the repo root.
