# Review Findings — Intel iGPU Support (tray.rs)

## Legend

- **NEW** — introduced by this diff
- **PRE-EXISTING** — was already in the codebase; this diff duplicates or exposes it
- **EXPOSED** — pre-existing code, but this diff creates new scenarios that trigger it

---

## 1. [NEW] `?` in RAPL loop exits function instead of continuing

- **File:** `src/razer-settings/tray.rs:742-743`
- **Severity:** High
- **What:** `parent()?` and `.ok()?.trim()` inside `for path in &rapl_candidates` return `None` from the entire `read_igpu_power()` function when a candidate fails, instead of `continue`-ing to the next candidate.
- **Fix:** Replace with `let-else continue`:
  ```rust
  let Some(parent) = std::path::Path::new(path).parent() else { continue; };
  let name_path = parent.join("name");
  let Ok(name_content) = fs::read_to_string(&name_path) else { continue; };
  if name_content.trim() != "uncore" { continue; }
  ```
- [x] Done

## 2. [PRE-EXISTING] GUI `get_igpu_power()` lacks "uncore" domain check

- **File:** `src/razer-settings/razer-settings.rs:441`
- **Severity:** Medium
- **What:** The GUI's RAPL code reads `intel-rapl:0:1` without verifying its `name` file says "uncore". The new tray code correctly checks this. On platforms where `:0:1` is "core", the GUI shows CPU core power as iGPU power.
- **Fix:** Port the tray's `name == "uncore"` guard to the GUI's `get_igpu_power()`.
- [x] Done

## 3. [EXPOSED] GUI iGPU row hidden when only utilization is available

- **File:** `src/razer-settings/razer-settings.rs:754`
- **Severity:** Medium
- **What:** `let igpu_has = igpu_temp.is_some() || igpu_pwr.is_some()` doesn't include `igpu_util`. Before this diff, Intel iGPU data was never populated, so this never mattered. Now the i915 freq-based util can be the only available metric — the tray shows "iGPU 42%" but the GUI hides the row.
- **Fix:** Change to `igpu_temp.is_some() || igpu_pwr.is_some() || igpu_util.is_some()`.
- [x] Done

## 4. [PRE-EXISTING] `SystemTime` used instead of `Instant` for RAPL deltas

- **File:** `src/razer-settings/tray.rs:699` (read_system_power), `:751` (read_igpu_power)
- **Severity:** Low
- **What:** `SystemTime::now()` is a wall clock subject to NTP backward steps. If the clock jumps back, `now - pt` wraps as u64 (near-zero reading in release, panic in debug). This existed in `read_system_power()` before; the diff copies it into `read_igpu_power()`.
- **Fix:** Use `Instant::now()` (monotonic). Store elapsed micros since first call in the AtomicU64.
- [x] Done

## 5. [PRE-EXISTING, worsened] RAPL delta logic duplicated 4 times

- **Files:** `tray.rs:696`, `tray.rs:748`, `razer-settings.rs:357`, `razer-settings.rs:444`
- **Severity:** Low
- **What:** Identical swap-compare-divide pattern with `AtomicU64` statics appears 4 times. The copies already diverge (GUI missing uncore check, different path lists). Any future fix must touch all 4.
- **Fix:** Extract a shared helper, e.g. `fn rapl_delta(energy: u64, last_e: &AtomicU64, last_t: &AtomicU64) -> Option<f64>`.
- [x] Done

## 6. [PRE-EXISTING] RAPL counter wraparound drops one reading

- **File:** `src/razer-settings/tray.rs:757` (and all other RAPL delta copies)
- **Severity:** Low
- **What:** `energy > pe` guard rejects wraparound but drops the entire reading. RAPL counters can wrap every ~60s at high power. `max_energy_range_uj` sysfs file provides the wrap point for correct modular arithmetic.
- **Fix:** Read `max_energy_range_uj` once (cache it) and compute `(energy + max - pe) % max` on wraparound.
- [x] Done
