# Phase 2 deployment — Uptime-Kuma services panel

Adds a second panel and a second sensor source. The host panel and its provider are
unchanged; this replaces the renderer unit so it knows about both panels.

```
aster-sysinfo ──▶ /run/asterctl/sensors/host.txt    (Phase 1, unchanged)
aster-prom    ──▶ /run/asterctl/sensors/kuma.txt    (new)
                          │
asterctl  ──rotates both panels──▶ LCD
```

## The credential

Kuma needs HTTP basic auth with an empty username and the API key as the password.
`aster-prom` reads it from a file rather than an argument, because a credential on the
command line is visible in `ps` to every local user and would end up in the unit file.

```shell
sudo install -d -m 0750 -o root -g asterctl /etc/asterctl
sudo install -m 0640 -o root -g asterctl \
    ~/.hermes/secrets/kuma-panel-metrics.key /etc/asterctl/kuma.key
```

Only the first line is used and the trailing newline is stripped.

## Sensor mapping — the fiddly part

`aster-prom` uses the **entire Prometheus metric line** as the sensor key, labels and all:

```
uptime_kuma_status{monitor_name="internet",monitor_url="https://example.com"}
```

A panel cannot reference that, and it would break silently if a monitor's URL changed in
Kuma. So the panel asks for stable names (`kuma_internet_status`) and
`cfg/sensor-mapping.cfg` resolves them.

Get the exact keys from the live endpoint and copy them **verbatim**, braces and all:

```shell
/opt/asterctl/bin/aster-prom "http://192.0.2.22:3001/metrics" \
    --password-file /etc/asterctl/kuma.key --console \
    | grep -E 'internet|opnsense|traefik|hindsight|cloudflare'
```

Append the ten mappings to `/opt/asterctl/cfg/sensor-mapping.cfg` — the template in
`cfg/sensor-mapping.cfg.example` has them commented out ready to fill in. Anything left
unmapped renders `--`, which is correct rather than broken: the sensor genuinely is not
there.

## Per-source max age

Kuma is a network call on a 30s refresh, so it needs more slack than the 3s local host
scrape. Create `/opt/asterctl/cfg/max-age.cfg`:

```
host:  10
kuma:  90
```

Without this both sources use the global `--max-age 10` and Kuma's values would flicker
to `--` between scrapes.

## Install

```shell
sudo cp -r panels/phase2-services /opt/asterctl/panels/
sudo cp cfg/panels.json /opt/asterctl/cfg/
sudo cp linux/phase2/aster-prom-kuma.service /etc/systemd/system/
sudo cp linux/phase2/asterctl-panel.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now aster-prom-kuma.service
sudo systemctl restart asterctl-panel.service
```

Edit the Kuma host in `aster-prom-kuma.service` first — it points at docker-host, not nas-host.

`cp -r` preserves the `fonts` symlink; copying the panel directory another way breaks it.

## Checks

```shell
systemctl status aster-prom-kuma
head -20 /run/asterctl/sensors/kuma.txt
journalctl -u asterctl-panel -n 20 --no-pager | grep -i 'Switching panel'
```

You should see the renderer alternating between `phase1-host` and `phase2-services`
every 30 seconds.

If the services panel shows `--` in every row, the mapping is wrong or missing — compare
the left-hand names in `sensor-mapping.cfg` against the labels in
`panels/phase2-services/panel.json`, and the right-hand keys against what `--console`
prints.

## Panel rotation cost

Switching panels changes the background, so it costs a near-full frame — about 1.3s on
this 12 Mbps link. At the 10s dwell we settled on, that is roughly 13% of the link.

That sounds like a lot and was the original reason for a 30s dwell, but it was the wrong
thing to optimise: the cost is 1.3s of visible wipe, and the benefit is that nothing on
this display holds still for long. Faster rotation is *also* the cheapest retention
countermeasure available, because no pixel sits at one value for more than ten seconds.

Below about 8s the wipe starts to occupy a quarter of the cycle and the panel reads as
permanently mid-refresh. That is the floor, not 30s.
