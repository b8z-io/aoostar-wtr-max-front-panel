# SPEC — Sensor staleness and honest degradation

**Status:** proposal, for review
**Depends on:** [`ARCHITECTURE.md`](ARCHITECTURE.md) §6
**Target:** first code change to this fork, before any panel artwork

---

## 1. Problem

Sensor values are read from watched `.txt` files into a shared
`Arc<RwLock<HashMap<String, String>>>` and are **only ever inserted or overwritten — never
expired** (`crates/asterctl/src/sensors.rs`, `read_key_value_file`).

If a provider dies, its last values remain in the map indefinitely. The renderer has no way
to distinguish a value written half a second ago from one written last Tuesday. The panel
displays stale numbers that look completely live.

This matters more than a normal caching bug, because a status panel is consulted precisely
when something is wrong — which is also when a provider is most likely to be dead.

Secondary problem: when a value **is** absent, `render_all_sensors` simply doesn't draw the
sensor (`crates/asterctl/src/render.rs`). A silently missing element is ambiguous with a
layout bug. Absence should be explicit.

---

## 2. Requirements

| # | Requirement |
|---|---|
| R1 | Every sensor value carries the time it was last written |
| R2 | Values older than a configurable max age are treated as **absent**, not as their last value |
| R3 | Absent and stale values render an explicit placeholder, not nothing |
| R4 | Stale rendering is visually distinct from a legitimate extreme reading — "unknown" must never look like "critical" |
| R5 | Max age is configurable per source, since a 3-second local scrape and a 60-second remote API have different expectations |
| R6 | Panels can display source health (how many providers are live) without new rendering code |
| R7 | Internally generated sensors (`DATE_*`) are never stale — they are computed at render time |
| R8 | Default behaviour is unchanged when the feature is not configured, so the change stays upstreamable |

---

## 3. Design

### 3.1 Timestamped values

Replace the map's value type:

```rust
pub struct SensorValue {
    pub value: String,
    pub updated: Instant,
    pub source: SourceId,
}

// crates/asterctl/src/sensors.rs
pub type SensorMap = HashMap<String, SensorValue>;
```

`SourceId` is a cheap interned handle for the originating file — `read_path` and the watcher
both know the path, so it is set at read time. Interning keeps `SensorValue` small and makes
per-source lookups trivial.

Every write to the map stamps `updated`. There is no other place values enter the map.

### 3.2 Staleness resolution

A single lookup helper, taking `now` as a parameter so it is unit-testable without a clock
abstraction:

```rust
impl SensorMap {
    fn resolve(&self, label: &str, now: Instant, cfg: &StalenessConfig) -> Option<&str>
}
```

Returns `None` when the entry is missing **or** when
`now - entry.updated > cfg.max_age_for(entry.source)`.

`render_all_sensors` uses `resolve` instead of `values.get(...)`. Its existing fallback chain
is preserved: resolved value → `get_date_time_value` → placeholder. R7 holds automatically,
since date/time sensors are computed at render time and never enter the map.

### 3.3 Rendering absent values

`render_sensor` gains a `state: ValueState` argument (`Live` | `Stale`).

| Mode | Live | Stale |
|---|---|---|
| Text | value + unit | `staleText` (default `--`), in `staleColor` |
| Fan | arc at value | arc at minimum, `staleColor`, no fill |
| Progress | bar at value | empty bar outline, `staleColor` |
| Pointer | needle at value | needle at minimum, `staleColor` |

Two optional fields are added to `Sensor`, both `#[serde(default)]` so existing AOOSTAR-X
JSON parses unchanged:

```jsonc
"staleText":  "--",        // string, default "--"
"staleColor": -8355712     // AOOSTAR-X colour int, default mid-grey
```

R4 is satisfied by colour channel separation: threshold colouring (green/amber/red) signals
the *value*; `staleColor` signals *no value*. They must not share a hue.

### 3.4 Configuration

New optional CLI arguments on `asterctl`:

```
--max-age <SECONDS>              Global default. Unset = feature off (R8).
--max-age-file <FILE>            Per-source overrides.
```

`--max-age-file` reuses the existing key-value config parser
(`read_key_value_file`), keyed by sensor file stem:

```
# <sensor file stem>: <seconds>
host:    10
homelab: 90
```

Resolution order: per-source override → global default → never stale.

### 3.5 Source health as internal sensors (R6)

Extend the internal-sensor mechanism that already backs `DATE_*`, adding a `SYS_` family
computed at render time. This reuses all existing sensor modes and needs no new rendering
code — a panel just references the label.

| Label | Value |
|---|---|
| `SYS_sources_total` | number of known sensor files |
| `SYS_sources_live` | number whose newest value is within max age |
| `SYS_sources_health` | `SYS_sources_live / SYS_sources_total` as a percentage — drives a Progress sensor directly |
| `SYS_source_<stem>_age` | seconds since that source last updated |
| `SYS_source_<stem>_live` | `1` or `0` — drives a status dot |

Implemented alongside `get_date_time_value` in `sensors.rs`, resolved in the same fallback
chain.

---

## 4. Test plan

Unit tests (`rstest` is already a dev-dependency):

1. Fresh value resolves; the same value with `updated` pushed past max age resolves to `None`
2. Per-source override wins over the global default
3. No max age configured → nothing is ever stale (R8 regression guard)
4. `DATE_*` labels resolve regardless of map contents and map age (R7)
5. `SYS_sources_live` counts correctly with a mix of fresh and stale sources
6. `SYS_sources_health` is 0 when every source is stale, 100 when all fresh
7. Stale text sensor renders `staleText`, not the last value

Visual verification with `--simulate --save`, comparing rendered PNGs:

8. Panel with all sources live
9. Same panel with one source stopped — stale elements show `--` in grey, clock still ticking
10. Same panel with every source stopped — all values `--`, clock still ticking,
    `SYS_sources_health` at 0

Tests 8–10 run headless with no hardware.

---

## 5. Non-goals

- Historical values or trends. The panel shows *now*.
- Alerting on staleness — Uptime-Kuma's job.
- Recovering or restarting providers. `asterctl` reports; systemd restarts.
- Per-*sensor* max age. Per-source is the right granularity; per-sensor is configuration
  surface nobody will maintain.

---

## 6. Upstreamability

Kept deliberately small and inert by default (R8) so it can be offered to
`zehnm/aoostar-rs` as a PR rather than becoming permanent fork drift. Constraints that
follow from that:

- No changes to the AOOSTAR-X JSON schema beyond additive `#[serde(default)]` fields
- No new required CLI arguments
- No new dependencies
- Behaviour identical to current `main` when `--max-age` is unset

---

## 7. Open questions 🔶

1. **Should a stale panel keep rotating?** If every source on panel 2 is dead, is it better
   to skip that panel and dwell on panel 1, or to show panel 2 full of `--` so the failure is
   visible? Leaning toward showing it — hiding failure contradicts the whole spec — but it's
   a judgement call worth arguing about.

   There's now a cost argument on the other side: `ARCHITECTURE.md` §5 shows a panel switch
   costs a near-full frame (~1.3 s of a ~700 KB/s link), so rotating into a dead panel spends
   real bandwidth to display nothing. It doesn't change the recommendation — a panel that
   quietly stops appearing is exactly the failure mode this spec exists to prevent — but if
   we skip, the skip must be *visible* on the panel we dwell on, not silent.
2. **Grace period on startup.** For the first N seconds after launch, nothing has been
   scraped yet and everything is legitimately absent. Suppress `--` during startup, or show
   it honestly? Leaning honest.
3. **Is `Instant` right, or should it be `SystemTime`?** `Instant` is monotonic and immune to
   clock steps, which is correct for age measurement, but it can't be serialised or logged as
   a wall-clock time. Probably `Instant` for logic, wall clock only for log lines.
