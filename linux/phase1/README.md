# Phase 1 deployment — pve-nas

Two processes, two units, no network, no persistent writes.

```
aster-sysinfo  --refresh 3 ──▶  /run/asterctl/sensors/host.txt   (tmpfs)
                                        │
asterctl  --sensor-path /run/asterctl/sensors  ──USB serial──▶  LCD
```

`aster-sysinfo` is a **separate binary**, not a flag on `asterctl`. Both units are
needed; neither does the other's job.

## Layout

Everything lives under `/opt/asterctl`, which keeps the `panels/*/fonts` symlinks
resolving without duplicating font files:

```
/opt/asterctl/bin/{asterctl,aster-sysinfo}
/opt/asterctl/cfg/phase1.json
/opt/asterctl/fonts/
/opt/asterctl/panels/phase1-host/{panel.json,img/,fonts -> ../../fonts}
```

## Install

Build on a machine with the toolchain (`pkg-config`, `libudev-dev`, and `protobuf-compiler`
once `aster-prom` is in play), then:

```shell
sudo install -d /opt/asterctl/bin
sudo install -m 0755 target/release/asterctl target/release/aster-sysinfo /opt/asterctl/bin/
sudo cp -r fonts cfg panels /opt/asterctl/
sudo cp linux/phase1/*.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now aster-sysinfo-host.service asterctl-panel.service
```

`cp -r panels` preserves the `fonts` symlink, which resolves to `/opt/asterctl/fonts`.
Copying the panel directory somewhere else on its own will break it.

## One host-specific step: the disk sensor

`aster-sysinfo` names disk sensors after the block device it found them on, so the
label differs on every machine. The panel asks for a stable `disk_root_usage_percent`
and a mapping file resolves it per host.

```shell
/opt/asterctl/bin/aster-sysinfo --console | grep usage_percent
```

Pick the device holding `/`, then:

```shell
sudo cp /opt/asterctl/cfg/sensor-mapping.cfg.example /opt/asterctl/cfg/sensor-mapping.cfg
sudo nano /opt/asterctl/cfg/sensor-mapping.cfg    # set the device path
sudo systemctl restart asterctl-panel
```

Until this is done ROOT DISK shows `--`. That is correct behaviour, not a fault: the
sensor the panel asked for genuinely does not exist, and the staleness layer is
refusing to invent one.

## Image retention

The panel arrived with the vendor logo ghosted into it — that image had been on screen
continuously since the machine was built. A sensor layout has the same problem more
slowly: fixed tiles, captions that never move, digits in the same place for years.

`--scrub-interval 60` runs a conditioning cycle once an hour: full white and black to
swing every subpixel between extremes, each primary to exercise the colour channels
separately, then noise to break up any spatial pattern. It takes roughly 25 seconds,
during which the panel shows the scrub rather than data.

Scrubs run **between panels, never mid-panel**, so they never interrupt a reading.

To disable, drop the flag. To watch one:

```shell
journalctl -u asterctl-panel -f | grep -i scrub
```

Existing retention usually fades over days once varied content is displayed. If it has
not improved after a week of scrubbing, the remaining option is a lighter background,
which hides a ghost better than a dark one does.

## Checks

```shell
# provider is writing, and the file is fresh
systemctl status aster-sysinfo-host
ls -l --time-style=full-iso /run/asterctl/sensors/host.txt
head -5 /run/asterctl/sensors/host.txt

# renderer found the panel by VID/PID, not by device node
journalctl -u asterctl-panel -n 30 --no-pager | grep -i 'serial port\|Display initialized'
```

## Verifying the honesty behaviour

The point of `--max-age 10` is that a dead provider stops being believed. Prove it
on the real panel:

```shell
sudo systemctl stop aster-sysinfo-host
# within ~10s every reading should fall to "--" in grey, while the clock keeps
# ticking and SOURCES LIVE drops to 0
sudo systemctl start aster-sysinfo-host
# readings return within one refresh
```

If values stay on screen after 10 seconds, staleness is not active — check that
`--max-age` is present in the running command line.

## Notes

- **`Wants=`, not `Requires=`.** If the provider dies the renderer keeps going and
  shows placeholders. A blank screen and a screen of stale numbers are both worse
  than a screen that admits it has no data.
- **`RestartSec=10` on the renderer is deliberate.** The frame cache lives in the
  process, so every restart costs a full-frame redraw — about 1.3s over a 12 Mbps
  link. A tight restart loop would saturate it.
- **`RuntimeDirectoryPreserve=yes`** on the provider stops `/run/asterctl` being
  removed when that unit is bounced, which would otherwise pull the directory out
  from under the renderer.
- Per-source max ages go in `/opt/asterctl/cfg/max-age.cfg` — see
  `cfg/max-age.cfg.example`. With one source, the global `--max-age` is enough.
