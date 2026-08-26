# Phase 3 deployment — the homelab panel

Adds the third panel and a third sensor source. Phases 1 and 2 are unchanged; this
replaces the renderer unit so it knows about all three panels.

```
aster-sysinfo ──▶ /run/asterctl/sensors/host.txt    (Phase 1, unchanged)
aster-prom    ──▶ /run/asterctl/sensors/kuma.txt    (Phase 2, unchanged)
aster-prom    ──▶ /run/asterctl/sensors/homelab.txt (new — from panel-metrics)
                          │
asterctl  ──rotates three panels──▶ LCD
```

## What the panel shows, and why it is mostly one number

Everything this homelab reports is a count of things that are fine: 69 monitors up,
5 pools online, 0 gateways down, 0 bad certificates. Laid out as a grid, the healthy
state and the state you must act on differ by one digit somewhere among six figures —
so you stop reading it within a week.

The headline inverts that. Healthy is a single green `0` filling a third of the glass;
anything else has a different *shape*, which you notice without reading. The six tiles
are the diagnosis, not the alarm — four map one-to-one onto the counters that feed the
headline, so a non-zero figure is always explained by exactly one tile that has changed.

`homelab_problems_total` is **withheld, not zeroed**, when TrueNAS, Kuma or OPNsense
fails to scrape: panel-metrics omits the metric entirely, the key goes stale, and the
panel draws `--`. A zero that means "blind" is the one lie this panel must never tell.

## Sensor mapping — two lines

Most of the Phase 3 metrics carry no Prometheus labels, so they need no mapping at all.
That is the payoff from aggregating upstream: `kuma_monitors_up` is already a panel-ready
name, where Phase 2 needed ten mappings to untangle `uptime_kuma_status{monitor_name=...}`.

Two are labelled and do need resolving. Append to `/opt/asterctl/cfg/sensor-mapping.cfg`:

```
opnsense_wan_gateway:    opnsense_gateway_up{gateway="WAN_GW"}
hass_speedtest_download: hass_entity_state{entity="sensor.speedtest_tracker_download",unit="Mbit/s"}
```

Copy the right-hand sides **verbatim** from the live endpoint, braces, quotes and all —
`aster-prom` uses the entire metric line as the sensor key:

```shell
/opt/asterctl/bin/aster-prom "http://192.168.68.22:9101/metrics" --console \
    | grep -E 'gateway_up|speedtest_tracker_download'
```

Anything left unmapped renders `--`, which is correct rather than broken: the sensor
genuinely is not there.

## Per-source max age

`homelab.txt` is a network scrape on a 30s refresh, and panel-metrics itself caches on its
own timer — so the value can legitimately be up to two refresh periods old. Add the third
line to `/opt/asterctl/cfg/max-age.cfg`:

```
host:     10
kuma:     90
homelab: 150
```

Without it, `homelab` inherits the global `--max-age 10` and every tile flickers to `--`
between scrapes.

## Install

On docker2, if the compose file moved to `./secrets` since you deployed:

```shell
cd /home/hermes-bot/panel-metrics
docker compose up -d --build
curl -s localhost:9101/metrics | grep homelab_
```

You want two lines: `homelab_problems_total 0` and `homelab_sources_down 0`. If the first
is missing, one of TrueNAS, Kuma or OPNsense is down — check `panel_metrics_source_up`.

On pve-nas:

```shell
sudo cp -r panels/phase3-homelab /opt/asterctl/panels/
sudo cp linux/phase3/aster-prom-homelab.service /etc/systemd/system/
sudo cp linux/phase3/asterctl-panel.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now aster-prom-homelab.service
sudo systemctl restart asterctl-panel.service
```

Edit the host in `aster-prom-homelab.service` first — it points at docker2, not pve-nas.

`cp -r` preserves the `fonts` symlink; copying the panel directory another way breaks it.

## Checks

```shell
systemctl status aster-prom-homelab
grep homelab /run/asterctl/sensors/homelab.txt
journalctl -u asterctl-panel -n 30 --no-pager | grep -i 'Switching panel'
```

The renderer should now cycle `phase1-host` → `phase2-services` → `phase3-homelab` every
30 seconds.

## Rotation cost

Three panels at a 30s dwell is a 90s cycle. Each switch changes the background, so it
costs a near-full frame — about 1.3s on this 12 Mbps link, or roughly 4% of it. Adding
the third panel does not change that percentage: the cost is per switch, and switches
still happen every 30s regardless of how many panels are in the rotation.

What it does change is how long you wait to see a given panel. 90s is a long time to wait
for the one number that says whether anything is wrong. If that becomes annoying, the fix
is to drop Phase 2 rather than to shorten `switchTime` — Phase 3's `MONITORS 69/69` covers
the same ground at lower resolution, and Phase 2 earns its slot only when you want the
per-service response times.

## Regenerating the artwork

```shell
python3 panels/phase3-homelab/generate.py
```

Needs Pillow and DejaVuSans (Debian: `python3-pil fonts-dejavu-core`). The generator
searches the panel's `fonts/` symlink first, then the usual system locations.
