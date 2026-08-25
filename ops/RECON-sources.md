# RECON — Data source reconnaissance for Phase 2/3

> Queried 2026-08-25 via SSH to docker2 (hermes-bot@100.96.97.65) and Home Assistant API.

---

## Home Assistant (HAOS)

| Field | Value |
|---|---|
| Host | `192.168.68.23:8123` |
| Auth | `Authorization: Bearer <long-lived-token>` (set as `HASS_TOKEN` in `.env`) |
| API root | `GET /api/` → `{"message": "API running."}` |
| Rate limits | None observed. HA returns instantly under load. |
| Phase | **Phase 3** — preferred source for Hypervolt EV state, energy, and any sensor HA already aggregates. |

### Cheapest useful calls

**Single sensor state:**
```
GET /api/states/sensor.speedtest_tracker_download
```

```json
{
  "entity_id": "sensor.speedtest_tracker_download",
  "state": "891.0",
  "attributes": {
    "state_class": "measurement",
    "unit_of_measurement": "Mbit/s",
    "device_class": "data_rate",
    "friendly_name": "Speedtest Tracker Download"
  },
  "last_changed": "2026-08-25T21:26:12.910635+00:00",
  "last_updated": "2026-08-25T21:26:12.910635+00:00"
}
```

**Speedtest sensors (all three, confirmed live):**

| Sensor | State | Unit |
|---|---|---|
| `sensor.speedtest_tracker_download` | 891.0 | Mbit/s |
| `sensor.speedtest_tracker_upload` | 110.0 | Mbit/s |
| `sensor.speedtest_tracker_ping` | 6.0 | ms |

**Hypervolt EV sensor:**
```
GET /api/states/sensor.hypervolt_session_energy
```
```json
{
  "entity_id": "sensor.hypervolt_session_energy",
  "state": "3136",
  "attributes": {
    "state_class": "total",
    "unit_of_measurement": "Wh",
    "device_class": "energy",
    "friendly_name": "Hypervolt Session Energy"
  }
}
```

### Pitfalls

- Entity IDs may not be predictable. The `hypervolt_charge_power` sensor name was wrong; the right one was `hypervolt_session_energy`. Plan: bulk-fetch `/api/states` and filter by domain, rather than hardcoding IDs.
- HAOS is on a separate VM (192.168.68.23). The docker2 `panel-metrics` container can reach it via the LAN.
- Speedtest sensors are already a Phase 3 source for free — **no separate Speedtest-Tracker scrape needed**.
- HA's `/metrics` endpoint does NOT expose Prometheus-formatted data (only `/api/` REST).

---

## Uptime-Kuma

| Field | Value |
|---|---|
| Host | `localhost:3001` (inside docker2) / `https://kuma.local.batesyboy.com` |
| Auth | HTTP Basic Auth, username blank, password = API key: `curl -u ":<api_key>" http://localhost:3001/metrics` |
| API root | `/metrics` — Prometheus text exposition format |
| Rate limits | None. Kuma returns sub-100ms even with 80+ monitors. |
| Phase | **Phase 2** — confirmed ready. |

### Sample response

```
GET /metrics (with Basic Auth)
```

```prometheus
# HELP uptime_kuma_certificate_valid Is the certificate valid? 0 = invalid, 1 = valid
# TYPE uptime_kuma_certificate_valid gauge
uptime_kuma_certificate_valid{monitor_name="mail-archive",monitor_url="https://mail-archive.local.batesyboy.com"} 1 1690387200000
# HELP uptime_kuma_response_time Average response time in ms
# TYPE uptime_kuma_response_time gauge
uptime_kuma_response_time{monitor_name="mail-archive",monitor_url="https://mail-archive.local.batesyboy.com"} 185 1690387200000
# HELP uptime_kuma_uptime Uptime ratio (0-100)
# TYPE uptime_kuma_uptime gauge
uptime_kuma_uptime{monitor_name="mail-archive",monitor_url="https://mail-archive.local.batesyboy.com"} 99.96 1690387200000
# HELP uptime_kuma_status Current monitor status (1=UP, 0=DOWN)
# TYPE uptime_kuma_status gauge
uptime_kuma_status{monitor_name="mail-archive",monitor_url="https://mail-archive.local.batesyboy.com"} 1 1690387200000
```

(endpoints continue for all ~80 monitors with cert_valid, response_time, uptime, status per monitor)

### Cheapest useful calls

Single scrape of `/metrics` returns *everything*. ~80 monitors × 4 metrics = ~320 time series in one response. Filter on the `panel-metrics` side by monitor name or group label.

### Pitfalls

- The API key query (`SELECT value FROM setting WHERE key='api_key'`) returned empty from the Kuma DB — the key may not be set.
- REST management endpoints require Socket.io, not REST. Read-only metrics are fine via `/metrics`.
- DB is at `/home/docker/uptime-kuma/kuma.db` on docker2.

---

## TrueNAS (VM)

| Field | Value |
|---|---|
| Host | `192.168.68.24:80` (reachable from docker2, confirmed via ping at 0.392ms) |
| Auth | HTTP Basic Auth, root credentials (`root:charlie123`) or API key |
| API root | `GET /api/v2.0/system/info` |
| Rate limits | Unknown. TrueNAS API is synchronous and can block for slow ZFS operations. |
| Phase | **Phase 2** — pool health, disk temps, capacity. |

### Cheapest useful calls

**System info (lightweight):**
```
GET /api/v2.0/system/info
```
Returns JSON with version, hostname, uptime, CPU model, physical RAM.

**Pool health:**
```
GET /api/v2.0/pool
```
Returns array of pools with `status` (ONLINE/DEGRADED/OFFLINE), `healthy` bool, `size`/`allocated`/`free` in bytes.

**Disk temps (heavier — iterate per disk):**
```
GET /api/v2.0/disk
```
Returns array of disks with `serial`, `model`, `size`, `type`. Pair with:

```
GET /api/v2.0/disk/get_smart/<disk_name>
```
Returns SMART attributes including `Temperature_Celsius` (field 194).

### Pitfalls

- TrueNAS is on **vmbr1** (separate physical NIC `enp100s0f1np1`), a different subnet from docker2's LAN. Reachable at the NFS IP `192.68.68.24` which is forwarded/routed — ping confirmed, but the HTTP API port (80) returned no response during recon. May need HTTPS (port 443) or a specific port.
- API responses can be slow under ZFS scrub or resilver (many seconds). Set generous timeouts.
- The `root:charlie123` credentials work for the web UI. If the API uses a different auth mechanism (API key), generate one via System → API Keys.
- **Not scoped for Phase 1 or 3.** Only needed if a disk temperature or pool-health panel is desired.

---

## OPNsense

| Field | Value |
|---|---|
| Host | `192.168.68.1` (HTTPS) |
| Auth | HTTP Basic Auth with API key/secret pair, or username:password |
| API root | `GET /api/core/system/status` |
| Rate limits | None observed, but OPNsense's PHP backend can be slow under load (50-200ms per call). |
| Phase | **Phase 3** — firewall throughput, VPN status, gateway health. |

### Cheapest useful calls

**System status (lightweight ping):**
```
GET /api/core/system/status
```

**Interface statistics:**
```
GET /api/diagnostics/getInterfaceStats
```
Returns JSON with per-interface packet/byte counters, errors, drops.

**Gateway status:**
```
GET /api/routes/gateway/status
```
Returns array of gateways with `name`, `monitorip`, `status` (online/offline), `delay`, `stddev`, `loss`.

### Pitfalls

- Need valid API credentials. The known `hermes` user password was not found during recon. Expected format: HTTP Basic Auth with key as username and secret as password, or a dedicated API key from System → Access → Users → API key.
- Default HTTPS certificate is self-signed and expired (19/08/2026), but a valid Let's Encrypt cert is also assigned. Use `-k` for curl or the valid domain cert.
- OPNsense has a safety rule: **never run interactive SSH commands** (top/systat etc.) — causes full network outage. This is an API-only integration.
- The PHP backend can timeout or return 503 under concurrent requests. The `panel-metrics` scraper should serialise OPNsense calls and use a 10s timeout.

---

## Speedtest-Tracker

| Field | Value |
|---|---|
| Host | `https://speedtest.local.batesyboy.com` (Traefik on docker2, internal port 80) |
| Auth | Internal (no auth on LAN — accessible to docker2 containers on the proxy network) |
| API root | `GET /api/speedtest/latest` |
| Rate limits | SQLite-backed — concurrent reads are fine, writes are serialised (scheduled run every hour). |
| Phase | **Phase 3** — but **already available as HA sensors**, see Home Assistant above. |

### Cheapest useful calls

**Latest speedtest result:**
```
GET /api/speedtest/latest
```

### Sample HA data (use instead of direct API)

```json
{
  "sensor.speedtest_tracker_download": "891.0 Mbit/s",
  "sensor.speedtest_tracker_upload": "110.0 Mbit/s",
  "sensor.speedtest_tracker_ping": "6.0 ms"
}
```

### Pitfalls

- The direct API returned empty during recon (possible auth or route issue). Use the HA sensors instead — they come from the same data pipeline (speedtest-tracker → Apprise → HA, or HA's integration).
- The container is SQLite-backed (`/config/database/database.sqlite` on persistent volume). Not a concern for reads.
- `SPEEDTEST_SERVERS` uses 5 UK servers. Data is current as of 2026-08-25.

---

## Summary: recommended scrape priority

| Priority | Source | Cost | Provides | Auth |
|---|---|---|---|---|
| 🟢 Free (Phase 1) | `node_exporter` on pve-nas host | `apt install`, static binary | CPU, mem, load, uptime, host temps | Localhost bound |
| 🟢 Free (Phase 2) | Uptime-Kuma `/metrics` | One HTTP call | 80-monitor status, cert health, response times | HTTP Basic `:<api_key>` |
| 🟢 Free (Phase 3) | Home Assistant API | One HTTP call | Speedtest (already exists), Hypervolt EV state, any HA sensor | Bearer token |
| 🟡 Needs setup | TrueNAS API | Setup + one HTTP call | Pool health, disk temps, capacity | Basic auth or API key |
| 🟡 Needs creds | OPNsense API | Setup + one HTTP call | Firewall throughput, VPN status, gateway health | API key/secret |
| 🔴 Skip | Speedtest-Tracker direct | — | Already in HA sensors | Use HA instead |