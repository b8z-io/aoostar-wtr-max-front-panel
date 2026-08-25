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
- **Claude:** this contradicts the Auth row above, which states the `:<api_key>` call is confirmed working. Both cannot be true, and it is load-bearing — the revised Phase 2 depends on Kuma being a working stock Prometheus source. Settle it with one `curl -u ":<key>" http://localhost:3001/metrics` and correct whichever line is wrong.
- REST management endpoints require Socket.io, not REST. Read-only metrics are fine via `/metrics`.
- DB is at `/home/docker/uptime-kuma/kuma.db` on docker2.

---

## TrueNAS (VM 247 on pve-nas)

> **Credentials found:** `~/.hermes/secrets/truenas-readonly.key`
> **Bearer token confirmed working against HTTPS.**

| Field | Value |
|---|---|
| Host | `https://192.168.68.24` (HTTPS on 443 — HTTP port 80 did not respond) |
| Auth | `Authorization: Bearer <token>` from `truenas-readonly.key` |
| API root | `GET /api/v2.0/system/info` |
| Rate limits | Unknown. TrueNAS API is synchronous and can block for slow ZFS operations. |
| Phase | **Phase 2** — pool health, disk temps, capacity. |

### Sample responses

**System info (lightweight, ~2KB):**
```
GET /api/v2.0/system/info
```
```json
{
  "version": "25.04.2.6",
  "hostname": "truenas",
  "physmem": 33658900480,
  "model": "AMD Ryzen 7 PRO 8845HS w/ Radeon 780M Graphics",
  "cores": 4,
  "physical_cores": 4,
  "loadavg": [0.0, 0.0, 0.0],
  "uptime_seconds": 375754,
  "timezone": "Europe/London",
  "system_product": "Standard PC (Q35 + ICH9, 2009)",
  "ecc_memory": true
}
```

**Pool health:**
```
GET /api/v2.0/pool
```
```json
[
  {
    "id": 1,
    "name": "vault",
    "status": "ONLINE",
    "path": "/mnt/vault",
    "topology": {
      "data": [{"name": "raidz1-0", "type": "RAIDZ1"}]
    },
    "scan": {
      "function": "SCRUB",
      "state": "FINISHED",
      "errors": 0,
      "percentage": 100.0
    }
  }
]
```

**Disk info (for temps):**
```
GET /api/v2.0/disk
```
Returns array of disks with `serial`, `model`, `size`, `type`, `tempt`. Pair with:
```
GET /api/v2.0/disk/get_smart/<disk_name>
```
Returns SMART attributes including `Temperature_Celsius` (field 194).

### Pitfalls

- **API uses HTTPS, not HTTP.** Port 80 returned nothing; HTTPS on 443 worked immediately.
- TrueNAS is on **vmbr1** (separate physical NIC `enp100s0f1np1`), but is reachable at `192.168.68.24` from docker2 at 0.392ms ping.
- Bearer token in `~/.hermes/secrets/truenas-readonly.key` is stable and working.
- API responses can be slow under ZFS scrub or resilver. The scrub just finished with 0 errors — good timing.
- Pool is a single RAIDZ1 vdev named `vault`. Single pool, no second pool visible.
- **Not scoped for Phase 1 or 3.** Only needed if a disk temperature or pool-health panel is desired.

---

## OPNsense

> **Credentials found:** `~/.hermes/secrets/opnsense-hermes.env`
> **API key + secret confirmed working via HTTP Basic Auth.**

| Field | Value |
|---|---|
| Host | `https://192.168.68.1` (HTTPS) |
| Auth | HTTP Basic Auth: key as username, secret as password |
| API root | `GET /api/core/system/status` |
| Rate limits | None observed. OPNsense's PHP backend returns responses in 50-200ms. |
| Phase | **Phase 3** — firewall throughput, VPN status, gateway health. |

### Sample responses

**System status (lightweight — always call first to auth-probe):**
```
GET /api/core/system/status
```
```json
{
  "metadata": {
    "system": {
      "status": 2,
      "message": "No pending messages",
      "title": "System"
    },
    "subsystems": []
  }
}
```
Status `2` = healthy. Status `0` = pending reboot. Status `1` = updates available.

**Gateway status:**
```
GET /api/routes/gateway/status
```
```json
{
  "items": [
    {
      "name": "WAN_GW",
      "address": "151.226.144.1",
      "status": "none",
      "loss": "~",
      "delay": "~",
      "stddev": "~",
      "monitor": "~",
      "status_translated": "Online"
    }
  ],
  "status": "ok"
}
```

**Interface statistics:**
```
GET /api/diagnostics/getInterfaceStats
```
Returns empty for the `hermes` read-only user (may need specific interface name or higher permissions). Alternative: `GET /api/interfaces/overview` or direct interface endpoint.

### Pitfalls

- **Credentials exist and work.** Found at `~/.hermes/secrets/opnsense-hermes.env` containing `API_KEY` and `API_SECRET`. Format: HTTP Basic Auth with key as username, secret as password.
- OPNsense has a safety rule: **never run interactive SSH commands** (top/systat etc.) — causes full network outage. This is an API-only integration.
- The PHP backend can timeout or return 503 under concurrent requests. The `panel-metrics` scraper should serialise OPNsense calls and use a 10s timeout.
- Some read-only endpoints may return empty for the `hermes` user if permissions don't cover them. Stick to `core/system/status`, `routes/gateway/status`, and traffic stats.
- Default HTTPS certificate is self-signed and expired (19/08/2026), but a valid Let's Encrypt cert is also assigned. Use `-k` or the valid domain cert.

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
| 🟢 Ready (Phase 2) | TrueNAS API | Bearer token in `secrets/` | Pool health, disk temps, capacity | Bearer token `~/.hermes/secrets/truenas-readonly.key` |
| 🟢 Ready (Phase 3) | OPNsense API | API key+secret in `secrets/` | Firewall throughput, VPN status, gateway health | HTTP Basic `key:secret` from `~/.hermes/secrets/opnsense-hermes.env` |
| 🔴 Skip | Speedtest-Tracker direct | — | Already in HA sensors | Use HA instead |