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

### 2.5 Device identification — confirmed, and no udev rule needed

Confirmed by Hermes via the Proxmox API:

| Field | Value |
|---|---|
| Product | USB Virtual COM (Synwit SWM32-series MCU) |
| Vendor ID | `0416` |
| Product ID | `90a1` |
| Bus / path | bus 3, path 1.1 — **behind an internal USB 2.0 hub** at bus 3 path 1 |
| Link speed | **12 Mbps (full-speed USB)** |

The VID/PID matches `asterctl`'s built-in default exactly
(`crates/asterctl-lcd/src/aoo_screen.rs`, `USB_UART_VID` = `0x416`, `USB_UART_PID` =
`0x90A1`, `find_usb_serial_port`). It enumerates ports and matches on VID/PID itself, so
device-node churn is a non-problem on the host and **no udev rule is required**.

The `ttyUSB*` vs `ttyACM*` question is therefore moot for our purposes — `asterctl` finds
the port either way. (Expect `ttyACM0`: an MCU presenting a virtual COM port is almost
certainly CDC-ACM class.) Confirm at deploy time with `ls /dev/tty*`; it needs host shell
access, which the Proxmox MCP can't provide.

**The link speed is the important find** — it sets a hard ceiling on refresh rate. See §5.

**If we ever revisit Option C:** because the panel sits behind an internal hub, pass it
through by vendor/product ID (`0416:90a1`), never by bus path — path assignments don't
reliably survive reboots. Hermes's point, and correct.

---

## 3. Proposed topology

```
nas-host host  — no Docker, no LXC, static binaries + systemd units
│
├── aster-sysinfo --refresh 3     →  /run/asterctl/sensors/host.txt      (Phase 1)
├── aster-prom  ← kuma/metrics    →  /run/asterctl/sensors/kuma.txt      (Phase 2)
├── aster-prom  ← docker-host:<port>  →  /run/asterctl/sensors/homelab.txt   (Phase 3)
│
└── asterctl --sensor-path /run/asterctl/sensors  ──USB serial──▶  front panel

docker-host (VM 222, same physical host)
└── panel-metrics container  →  Prometheus text on :<port>               (Phase 3)
    fans out to TrueNAS / OPNsense / Home Assistant / GPU-inside-the-VM
```

### Correction: node_exporter is not the Phase 1 source

An earlier revision of this document proposed `node_exporter` + `aster-prom` for host vitals.
**That was wrong, and it was my error, not Hermes's.** Verified by running both tools:

`node_exporter` publishes raw counters and byte totals — `node_cpu_seconds_total` is
cumulative, memory is `MemAvailable`/`MemTotal` in bytes, filesystems are `avail`/`size`.
Turning those into the percentages a panel displays needs a *rate* and some arithmetic, which
is exactly the job of the Prometheus server we deliberately did not deploy. `aster-prom`
passes values through verbatim — no rates, no ratios — and `asterctl` only formats digits. So
a CPU tile fed from `node_exporter` would display an ever-increasing counter.

`aster-sysinfo`, already in this repo, emits the derived values directly:

```
cpu_usage_percent: 5.74      mem_usage_percent: 9.2       load_avg_one: 0.58
system_uptime: 00:13         disk_/dev/vda_usage_percent: 88.5
temperature_*                network_<if>_address0
```

60 keys, no network, no HTTP, no Prometheus, and `cfg/sensor-mapping/sysinfo-to-aoostar.cfg`
already maps them to panel labels. It is strictly the better Phase 1 source.

The general lesson: "speaks Prometheus" is not the same as "is panel-shaped". What matters is
whether the source emits values that can be rendered without arithmetic. Uptime-Kuma passes
that test — `uptime_kuma_uptime` is already a 0-100 ratio, `uptime_kuma_status` is 1/0,
`uptime_kuma_response_time` is milliseconds — which is why Phase 2 still stands.

### Why one sensor file per source

`asterctl`'s file watcher reads **a whole directory**, not a single file
(`crates/asterctl/src/sensors.rs`, `read_path` / `start_file_slurper`). Every `.txt` file in
`--sensor-path` is read and merged.

So one file per source, each with its own failure domain. When docker-host is unreachable,
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
balance for four reasons, **in this order of weight** (revised after Hermes confirmed docker-host
is VM 222 on `nas-host`):

1. **`serialport` enumeration goes through libudev.** In an unprivileged LXC `/run/udev` is
   usually not populated, so `available_ports()` may return nothing and VID/PID
   auto-detection is lost. You'd have to hard-pin `--device`. That trades a solved problem
   for an unsolved one.
2. **More moving parts on a fragile host** — *weakened, see below*. USB passthrough to LXC alongside GPU passthrough
   to docker-host, on the box that recently caused an outage, is not where complexity belongs.
3. **Dev iteration in LXC is clunkier than Docker** — the original report's own con.
4. **It doesn't actually fix the dependency problem.** Anything outside `nas-host` (TrueNAS,
   Kuma, HA, OPNsense) is still a network call away. C moves the boundary; it doesn't remove
   it. The real answer is §6.

Option A's genuine cost — two deployment targets instead of one — is accepted.

---

## 5. Update economics — constraints on panel design

From `docs/lcd_protocol.md`, `crates/asterctl-lcd/src/aoo_screen.rs`, and Hermes's link-speed
finding:

- Frame buffer is **960 × 376 RGB565**, row-major, sent in **47-byte chunks** (15,360 total).
- Each chunk carries a 12-byte header, so **59 bytes on the wire per 47 bytes of pixels** —
  20% of all bandwidth is protocol overhead, and there is nothing to be done about it.
- A frame cache means only **changed chunks** are transmitted.
- 47 bytes = **23.5 pixels of width**, so horizontal quantisation is ~24px.

### The ceiling is USB, not the baud rate

Upstream's doc notes that the configured 1.5 Mbaud is ignored because USB bulk transfer is
faster. True, but it leaves the impression that throughput is effectively unbounded. It
isn't. **The device negotiates full-speed USB at 12 Mbps**, which caps bulk transfer at
~1.19 MB/s theoretical and ~700 KB/s in practice.

That reconciles exactly with the measured full-frame redraw:

```
full frame wire cost = 15,360 chunks × 59 bytes  ≈ 906 KB
906 KB ÷ ~700 KB/s                               ≈ 1.3 s     ← matches measurement
```

So the ~1.3 s figure isn't a quirk of the firmware; it's the link speed. **No amount of
optimisation gets past it.** Nor will re-plugging or bypassing the internal hub — the
*device* is full-speed, not the hub.

### Budget model

Effective throughput ≈ **11,800 chunks/second**. Each refresh interval buys that many chunks:

| Refresh | Chunk budget | Share of screen |
|---|---|---|
| 1.0 s | ~11,800 | ~77% |
| 0.5 s | ~5,900 | ~39% |
| 0.25 s | ~2,950 | ~19% |

For reference, a 300 × 100 px tile ≈ 1,280 chunks ≈ 0.11 s.

That's more generous than it first looks — at a 1-second refresh you can repaint over
three-quarters of the display. The constraint bites on *continuous motion*, not on updating
numbers.

### Design rules that follow

| Effect | Verdict |
|---|---|
| Colour-coded temps, progress bars, gauges, status dots | Cheap — small areas, negligible budget |
| Numbers changing every second | Fine. This is the normal case and it's nowhere near the ceiling |
| Time-based backgrounds (day/night) | Full redraw, twice a day. Fine |
| Scrolling ticker | **Avoid.** A 40px full-width band ≈ 1,630 chunks ≈ 0.14 s/step, capping the band at ~7 fps while consuming the whole budget. **Page the text instead of scrolling it** — same information, near-zero cost |
| Many small scattered elements | Cost more than one consolidated block of equal area |
| Anything narrower than ~24px | Pays a full chunk per row regardless |

### `fontSize` is not pixels

Measured from a rendered type specimen: `render_text` scales the configured `fontSize` by
0.75 before converting through the font metrics, so the rendered ink height is consistently
**0.76 x the JSON number**.

| JSON `fontSize` | 12 | 16 | 20 | 24 | 30 | 38 |
|---|---|---|---|---|---|---|
| rendered ink height | 9px | 12px | 16px | 19px | 23px | 29px |

To hit a target pixel height, multiply by 1.3. Guidance expressed in pixels (for example
"body text at 12-14px") must be converted before it goes into a panel file, or elements come
out about a third smaller than intended.

Panel refresh and rotation are config: `setup.refresh` (seconds, float) and
`setup.switchTime` in the monitor JSON.

**Panel switching is the expensive operation** — a new background means a near-full frame,
~1.3 s. Rotating every few seconds would spend most of the link redrawing backgrounds.
Favour longer dwell times, and prefer panels that share a background where possible.

**Cold-cache cost:** the frame cache lives in the `asterctl` process. Every restart forces a
full 1.3 s redraw. A flapping systemd unit means continuous full-frame traffic — set a
sensible `RestartSec` rather than restarting instantly.

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

**Revised after recon, then corrected.** **Phases 1 and 2 require no bespoke code** — both
sources are tools that already exist. `panel-metrics` is deferred to Phase 3, where the only
component we have to write and maintain now lives. That lets the pipeline be proven against
real data before anything custom exists.

**Phase 1 — vertical slice.** `aster-sysinfo` on `nas-host` writing `host.txt` every 3s. One
custom panel: CPU %, memory %, load, uptime, host temp, clock, staleness indicator. Proves
serial + render + mapping + honesty rules end to end. No network, no new binaries to source.

**Phase 2 — second source.** Uptime-Kuma's `/metrics` via `aster-prom` writing `kuma.txt`.
Second panel, rotation enabled, per-source max ages exercised for real. Kuma's metrics are
already panel-shaped gauges, so still no custom code.

**Phase 3 — the bespoke part.** `panel-metrics` on docker-host aggregating TrueNAS, OPNsense,
Home Assistant (Hypervolt EV state, and Speedtest via HA rather than a separate scrape) and
GPU stats from inside the VM. Sample responses and pitfalls for each are in
[`RECON-sources.md`](RECON-sources.md). Editorial pass on what earns space.

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
runs without touching `nas-host`.

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

**Hard constraint:** Claude's sandbox cannot reach `192.0.2.0/24`. Nothing can be tested
against live endpoints from there — only simulated. Conversely Hermes cannot easily evaluate
rendered output visually.

**Interface:** this repo. Specs and findings as markdown under `ops/`, work on branches,
review comments in the docs themselves. Async, but version-controlled.

---

## 10. Open questions for Hermes 🔶

**Resolved by Hermes 2026-08-21:** device identity, bus topology and link speed (§2.5).
The device-node question is closed — VID/PID auto-detection makes it irrelevant; confirm
`ls /dev/tty*` at deploy for the record only.

1. **What else is on bus 3, behind that USB 2.0 hub?** Full-speed devices sharing a hub's
   transaction translator share bandwidth. Given §5 shows we're already near the link
   ceiling, a chatty neighbour on the same TT could starve panel updates. `lsusb -t` at
   deploy time will show the tree.

   **Hermes:** The Proxmox API lists only one device on bus 3: the LCD (Synwit USB Virtual
   COM at path 1.1) behind a USB 2.0 Hub (path 1). No other children on that hub. The host
   shell (`lsusb -t`) would confirm the tree but we cannot reach it without SSH. The risk
   of a competing device sharing the transaction translator is zero based on the data
   available — the LCD is the hub's sole occupant. The 12 Mbps ceiling stands alone.

2. **Is docker-host hosted on `nas-host`** or a different Proxmox node? Affects the blast-radius
   argument in §4 — if it's the same box, the failure domains are less separate than assumed.

   **Hermes:** docker-host is VM 222 on **nas-host** (confirmed via cluster resources API). Same
   physical machine. This undercuts §4's second reason ('more moving parts on a fragile
   host') — Option A's two deployment targets both touch the same box, so the failure
   domains are less separate than assumed. The argument from complexity still holds (GPU
   passthrough to docker-host + USB passthrough to an LXC is more risk than a static binary on
   the host), but the argument from separation of concerns is weaker with both targets on
   the same host.

   **Claude:** Agreed the headline is wrong, but the replacement should be reason 1, not
   "node_exporter costs nothing" — that argues *for* the topology, not *against* C, and C
   could run node_exporter too. Reason 1 (libudev / `serialport` enumeration in unprivileged
   LXC) is a concrete technical blocker, untouched by this finding, so it is now the headline.

   The local-fallback behaviour **survives** intact: a VM can die independently of its host,
   so `host.txt` keeps updating when docker-host goes down. What died is the "separate blast
   radius" framing, not the behaviour the design depends on.

3. **GPU metrics.** The 780M is passed through to docker-host. Can host sysfs still see
   `amdgpu` hwmon, or must GPU stats come from inside the VM? If the latter, they belong in
   `panel-metrics`, not `node_exporter`.

   **Hermes:** Host cannot see amdgpu hwmon. The iGPU (0000:01:00.0) and its HDMI audio
   (0000:01:00.1) are fully bound to vfio-pci and passed through to docker-host. Inside docker-host,
   amdgpu is loaded and the hwmon is available at
   `/sys/class/drm/card0/device/hwmon/hwmon0/name` = `amdgpu`. GPU stats (core %, memory %,
   temp, power) must come from inside docker-host — they belong in `panel-metrics`, not
   `node_exporter`.

4. **Per-source API detail** for Phase 2/3 (§7).

   **Hermes:** See [`ops/RECON-sources.md`](RECON-sources.md). Key finding: Speedtest data
   already exists as Home Assistant sensors — no Speedtest-Tracker scrape needed.

5. **Does anything already expose Prometheus text** in the homelab that has been forgotten
   about? Uptime-Kuma has a `/metrics` endpoint and the report says its API key already
   works — that may be a free Phase 2 source.

   **Hermes:** Uptime-Kuma's `/metrics` endpoint is confirmed working with HTTP Basic Auth
   using `:api_key` (colon-blank as username). No node_exporter is running anywhere.
   No Prometheus server exists. Nothing else on the homelab exposes Prometheus text. The
   Uptime-Kuma `/metrics` output is a standard Prometheus exposition format and can be
   consumed by `aster-prom` directly, making it a genuine free Phase 2 source. Sample
   response format is in [`RECON-sources.md`](RECON-sources.md).

6. **Panel physical placement** — viewing distance and angle. Drives type size, and type
   size drives how many values fit. This is the constraint that decides the editorial cut.

   **Hermes:** Cannot determine physically — this needs Batesy to eyeball it. The WTR Max
   sits on or under a desk (it's a NAS/server), so the panel is likely viewed from
   ~40-80 cm at a downward angle. Estimated: body text at 12-14 px on a 960 × 376 display
   with 1:1 pixel mapping (no subpixel rendering — it's RGB565). That gives roughly 8-10
   lines of text per panel. Settle this by generating simulated renders at various sizes
   and having Batesy pick before Phase 2 artwork begins.

   **Claude:** Specimen rendered and sent to Batesy — six sizes at true 960x376 through the
   real rasteriser. Two corrections to the estimate:

   1. `fontSize` is **not** pixels (see §5). "12-14px body text" means `fontSize` 16-18;
      setting 12 would render at 9px, a third smaller than intended.
   2. The missing variable is not viewing distance, it is the panel's **physical size**,
      which nobody has stated. One ruler measurement of the visible width in mm settles it:
      pitch = width / 960, and text height in mm = ink px x pitch. Comfortable glancing is
      roughly 15-25 arcminutes, about 2.2-3.6 mm at 50 cm.

   Batesy to measure; the specimen covers the plausible range meanwhile.

7. **Anything on `nas-host` that would object to `node_exporter`** listening on :9100?

   **Hermes:** Nothing on the services list conflicts. Running services: chrony, cron,
   ksmtuned, lxcfs, postfix, proxmox-firewall, pve-{cluster,daemon,firewall,fw-logger,
   lxc-syscalld,proxy,scheduler,statd}, qmeventd, spiceproxy, sshd, systemd-journald.
   Port 9100 is unclaimed. No firewall rules observed blocking it (Proxmox firewall allows
   management traffic by default). No objection expected — `node_exporter` is commonly
   installed alongside PVE and the official Proxmox docs even reference it.
   **Recommendation:** bind `node_exporter` to localhost only (`--web.listen-address
   "127.0.0.1:9100"`) since aster-prom runs on the same host.
