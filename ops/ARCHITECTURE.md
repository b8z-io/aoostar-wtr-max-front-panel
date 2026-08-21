# Front Panel — Architecture Brief

**Status:** proposal, for review
**Audience:** Hermes (homelab agent), Batesy (conductor)
**Date:** 2026-08-21

> This directory (`ops/`) holds fork-specific planning docs. It is deliberately outside
> `docs/`, which upstream builds into an mdBook — keeping our notes here avoids merge
> conflicts when we pull from `zehnm/aoostar-rs`.

**Hermes: please review and push back.** Sections marked 🔶 are open questions where your
knowledge of the live homelab beats anything that can be inferred from the code.

---

## 1. Decision summary

Build a refined version of **Option A** (split scraper / thin host renderer), with three
changes that remove most of the proposed work:

1. **`panel-metrics` emits Prometheus text format**, not bespoke JSON. The host-side fetcher
   then already exists — see §2.1.
2. **No Prometheus server.** We need the exposition *format*, not the time-series database.
   Zero disk writes, nothing to maintain. See §2.2.
3. **Sensor files live on tmpfs (`/run`).** The pipeline touches no persistent storage at all.

Rejected: Option C (single LXC). Reasoning in §4.

---

## 2. Corrections to the original spitball

These are the points where the original report was working from stale or incomplete
information. Each is verifiable in the tree.

### 2.1 The host-side fetcher already exists

Upstream has an **unmerged branch `feat/node-exporter`** (2 commits, Sept 2025) adding a
crate `aster-prom`: a Prometheus scraper that writes asterctl sensor files on a refresh loop.

- Handles both text and protobuf exposition formats
- TLS client certs, connect/total timeouts, `--refresh` loop
- Writes atomically: tempfile + `persist()` rename, `0o664` perms
  (`crates/aster-prom/src/main.rs`, `write_sensor_file`)
- ~2,200 lines that we do not have to write

So the "tiny fetcher on the host" in the original Option A is a stock binary, provided the
metrics endpoint speaks Prometheus text. That is a trivial constraint — it's a plain-text
HTTP response.

**Caveat:** `aster-prom` builds with `prost-build`, so the build host needs `protoc`.

### 2.2 Prometheus format ≠ Prometheus server

The concern about SSD/NVMe wear applies to the **server** (a TSDB that scrapes and stores).
We need only the **format**: a stateless HTTP endpoint listing current values. No database,
no retention, no writes.

A front panel displays *now*. It has no use for history. `node_exporter` alone — a single
static binary with no state — gives us host vitals in exactly the shape `aster-prom` wants.

### 2.3 Sensor mode support is better than reported

The original report said fan/progress/pointer modes were "partially working" and only text
mode was reliable. That was true of **v0.1.0**. `CHANGELOG.md` shows v0.2.0 fixed both known
defects:

- `#11` misplaced text sensors in custom panels
- `#12` wrong start position for counter-clockwise circular progress (fan) sensors

Gauges and progress bars should be usable.

### 2.4 Build from `main`, not the v0.2.0 release

Four commits landed after the v0.2.0 tag, and one of them matters a great deal:

| Commit | What |
|---|---|
| `7ac543e` | **sensor identifier mapping** |
| `f7ee6b0` | `cpu_usage_percent` + `system_uptime` in `aster-sysinfo` |
| `972f7e1` | internal date/time sensors |
| `9af5deb` | sensor filter option |

**Sensor identifier mapping** is the feature that makes arbitrary metric names usable. It
rewrites panel sensor labels to provider labels at config load
(`crates/asterctl/src/cfg.rs`, `set_sensor_mapping` / `include_custom_panel`), so we can feed
`node_exporter`-shaped names into panels without renaming anything upstream.

Format is `panel_label: provider_label`, one per line — see
`cfg/sensor-mapping/sysinfo-to-aoostar.cfg`. A sibling `<name>-filter.cfg` holds one regex
per line to drop unwanted keys.

**Attention:** `set_sensor_mapping` may only be called once at startup, and the original
labels are not preserved. Dynamic remapping is not supported.

### 2.5 No udev rule needed on the host

`asterctl` enumerates serial ports and matches on VID/PID itself, defaulting to exactly
`0416:90A1` (`crates/asterctl-lcd/src/aoo_screen.rs`, `USB_UART_VID` / `USB_UART_PID`,
`find_usb_serial_port`). Device-node churn is a non-problem when running on the host.

🔶 **Hermes:** the report says `/dev/ttyUSB0`. Synwit VCP is CDC-ACM, and upstream docs say
`/dev/ttyACM0`. Please confirm what actually appears on `pve-nas`. It shouldn't matter given
VID/PID auto-detection, but it's worth knowing.

---

## 3. Proposed topology

```
pve-nas host  — no Docker, no LXC, three static binaries + three systemd units
│
├── node_exporter                          :9100/metrics   stateless, zero writes
│
├── aster-prom  ← localhost:9100      →  /run/asterctl/sensors/host.txt
├── aster-prom  ← docker2:<port>      →  /run/asterctl/sensors/homelab.txt
│
└── asterctl --sensor-path /run/asterctl/sensors  ──USB serial──▶  front panel

docker2 (VM)
└── panel-metrics container  →  Prometheus text on :<port>
    fans out to TrueNAS / Uptime-Kuma / Home Assistant / OPNsense / Speedtest-Tracker
```

### Why two `aster-prom` instances

`asterctl`'s file watcher reads **a whole directory**, not a single file
(`crates/asterctl/src/sensors.rs`, `read_path` / `start_file_slurper`). Every `.txt` file in
`--sensor-path` is read and merged.

So one file per source, each with its own failure domain. When docker2 is unreachable,
`host.txt` keeps updating and the panel still shows live local vitals. The local-fallback
behaviour falls out of the design rather than needing to be built.

### Implementation notes

- **Output files must end in `.txt`.** The watcher explicitly skips other extensions.
- **`--temp-dir` must be on the same filesystem as the output** — the write is an atomic
  rename. With sensors on `/run`, use `--temp-dir /run/asterctl/tmp`.
- The watcher reacts to `ModifyKind::Data` and `ModifyKind::Name(RenameMode::To)`, so the
  atomic-rename write pattern is detected correctly.
- All source files merge into **one flat `HashMap<String, String>`**. Keys are global — two
  sources emitting the same key will collide. Namespace metric names per source.
- Units use a `<label>#unit` key convention (`crates/asterctl/src/render.rs`).

---

## 4. Why not Option C (single LXC)

The LXC option is coherent and the USB-passthrough pattern is documented. Rejected on
balance for four reasons:

1. **`serialport` enumeration goes through libudev.** In an unprivileged LXC `/run/udev` is
   usually not populated, so `available_ports()` may return nothing and VID/PID
   auto-detection is lost. You'd have to hard-pin `--device`. That trades a solved problem
   for an unsolved one.
2. **More moving parts on a fragile host.** USB passthrough to LXC alongside GPU passthrough
   to docker2, on the box that recently caused an outage, is not where complexity belongs.
3. **Dev iteration in LXC is clunkier than Docker** — the original report's own con.
4. **It doesn't actually fix the dependency problem.** Anything outside `pve-nas` (TrueNAS,
   Kuma, HA, OPNsense) is still a network call away. C moves the boundary; it doesn't remove
   it. The real answer is §6.

Option A's genuine cost — two deployment targets instead of one — is accepted.

---

## 5. Update economics — constraints on panel design

From `docs/lcd_protocol.md` and `crates/asterctl-lcd/src/aoo_screen.rs`, verified against a
simulated render:

- Frame buffer is **960 × 376 RGB565**, row-major, sent in **47-byte chunks** (15,360 total).
- A frame cache means only **changed chunks** are transmitted.
- A full-screen redraw is **~1.3 s**. Cost scales with chunks touched.
- 47 bytes = **23.5 pixels of width**, so horizontal quantisation is ~24px.

Design rules that follow:

| Effect | Verdict |
|---|---|
| Colour-coded temps, progress bars, gauges, status dots | Cheap — small areas |
| Time-based backgrounds (day/night) | Full redraw, but twice a day. Fine |
| Scrolling ticker | **Avoid.** A 40px full-width band is ~11% of the frame ≈ 0.14 s/step, capping the band at ~7 fps and eating the update budget. **Page the text instead of scrolling it** |
| Many small scattered elements | Cost more than one consolidated block of equal area |
| Anything narrower than ~24px | Pays a full chunk per row regardless |

Panel refresh and rotation are config: `setup.refresh` (seconds, float) and
`setup.switchTime` in the monitor JSON.

---

## 6. The honesty requirement

**The panel's most valuable moment is when the homelab is broken** — which is exactly when a
naive design lies to you.

Verified problem: sensor values are only ever inserted or overwritten in the shared map, and
**never expire** (`crates/asterctl/src/sensors.rs`). If a scraper dies, its last values stay
in the map indefinitely and render as though live. There is no staleness handling upstream.

The contract, agreed with Batesy:

1. **Every panel carries a live clock.** Free — `asterctl` generates date/time sensors
   internally, so they tick even when every scrape is dead. Instantly distinguishes
   "renderer alive, data stale" from "everything dead".
2. **Per-source max age.** Values older than N seconds render as `--`, never the last-known
   number.
3. **Missing means missing.** No interpolation, no hold, no last-good.
4. **Separate colour channels** for "hot" and "unknown", so a red CPU is never confused with
   a dead sensor.
5. **A source-health indicator** — a dot per source, or "3/4 live".

Points 2–4 require code. Spec in [`SPEC-staleness.md`](SPEC-staleness.md). This is proposed
as the **first change to the fork**, before any panel artwork.

---

## 7. Data sources — phased

The original inventory lists nine sources and ~30 metrics. The panel is 960 × 376 and fits
roughly 6–10 values legibly per panel. The list is a wishlist, not a plan; each source is an
integration with its own auth and failure mode.

**Phase 1 — vertical slice.** `node_exporter` on `pve-nas` only. One custom panel: CPU,
memory, load, uptime, host temp, clock, staleness indicator. Proves serial + scrape + render
+ honesty rules end to end.

**Phase 2.** Add `panel-metrics` with TrueNAS (pool health, disk temps, capacity) and
Uptime-Kuma (monitors up/down). Second panel, rotation enabled.

**Phase 3.** Speedtest-Tracker, OPNsense, Home Assistant (EV charge as a progress sensor),
Docker counts. Editorial pass on what earns space.

**Phase 4.** Design polish — custom backgrounds, gauges, day/night themes.

🔶 **Hermes:** for each Phase 2/3 source, what does the API *actually* return, and what's the
cheapest call that gets it? Rate limits, auth style, latency, anything that misbehaves under
load. Speedtest-Tracker and Dockhand especially — those are the least standard.

---

## 8. Build and deploy notes

```shell
# build deps (Ubuntu/Debian)
sudo apt install build-essential git pkg-config libudev-dev
# additionally, for aster-prom:
sudo apt install protobuf-compiler

cargo build --release          # ~2m15s cold, Rust 1.95
```

Rust 1.88 minimum, edition 2024. Verified: clean release build, 45 unit tests pass,
3 clippy warnings.

**Headless development works.** `asterctl --simulate` renders full panels with no hardware
attached:

```shell
./target/release/asterctl --simulate --config monitor.json --save
# writes rendered PNGs to ./out/
```

This is the single most valuable property of the codebase — the entire visual design loop
runs without touching `pve-nas`.

### ⚠️ Fork gotcha: disable the release workflow first

`.github/workflows/build.yml` has a `release` job that fires on **any push to `main`**. It
runs `gh release delete latest --cleanup-tag -y` and republishes. On this fork that will
start cutting GitHub releases under `b8z-io`. Gate or remove it before the first push to
`main`.

---

## 9. Proposed work split

| Who | What | Why |
|---|---|---|
| **Claude** | Rust changes, panel JSON, background artwork, visual iteration | Can run the full `--simulate` render loop headless and *look* at the output |
| **Hermes** | Live endpoint reconnaissance, deployment, systemd units, running against real hardware | Has the credentials, the network and the serial port |
| **Batesy** | Conductor; carries specs and findings between the two | — |

**Hard constraint:** Claude's sandbox cannot reach `192.168.68.0/24`. Nothing can be tested
against live endpoints from there — only simulated. Conversely Hermes cannot easily evaluate
rendered output visually.

**Interface:** this repo. Specs and findings as markdown under `ops/`, work on branches,
review comments in the docs themselves. Async, but version-controlled.

---

## 10. Open questions for Hermes 🔶

1. **Device node** on `pve-nas` — `ttyACM0` or `ttyUSB0`? (§2.5)
2. **Is docker2 hosted on `pve-nas`** or a different Proxmox node? Affects the blast-radius
   argument in §4 — if it's the same box, the failure domains are less separate than assumed.
3. **GPU metrics.** The 780M is passed through to docker2. Can host sysfs still see
   `amdgpu` hwmon, or must GPU stats come from inside the VM? If the latter, they belong in
   `panel-metrics`, not `node_exporter`.
4. **Per-source API detail** for Phase 2/3 (§7).
5. **Does anything already expose Prometheus text** in the homelab that has been forgotten
   about? Uptime-Kuma has a `/metrics` endpoint and the report says its API key already
   works — that may be a free Phase 2 source.
6. **Panel physical placement** — viewing distance and angle. Drives type size, and type
   size drives how many values fit. This is the constraint that decides the editorial cut.
7. **Anything on `pve-nas` that would object to `node_exporter`** listening on :9100?
