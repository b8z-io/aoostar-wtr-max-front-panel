#!/usr/bin/env python3
"""Generate the Phase 3 homelab panel: background artwork plus panel.json.

One headline figure fed by `panel-metrics`, with the six readings that explain it.

Design notes
------------
- THE PROBLEM WITH THE OBVIOUS PANEL. Every figure this homelab produces is a
  count of things that are fine: 69 monitors up, 5 pools online, 0 gateways
  down, 0 bad certificates. Laid out as a grid, the healthy state and the state
  you must act on differ by one digit somewhere among six numbers — so you stop
  reading it within a week. The headline inverts that. Healthy is a single
  green `0` occupying a third of the glass, and *any* other value has a
  different shape. You are reading a silhouette, not parsing digits.
- The tiles are the diagnosis, not the alarm. Four of them map one-to-one onto
  the counters that feed the headline (pools, monitors, certificates, gateway),
  so a non-zero headline is always explained by exactly one tile that has
  changed. The remaining two are readings rather than faults.
- SO THE TILES ARE MOSTLY UNCOLOURED, AND NEVER GREEN. Green belongs to the
  headline alone: it is the reassurance signal, and a panel with six green
  things on it has diluted the one that matters. Tiles turn amber or red where
  the value has a scale of its own — days to expiry, a gateway up or down — and
  stay neutral otherwise. Neutral here means "no intrinsic scale", never
  "fine", which is why colouring "5" green would be a lie: the panel does not
  know that 5 is all of them.
- Nothing here does arithmetic. `truenas_pools_online` exists because the panel
  renders "5 / 5" and cannot subtract; the aggregation layer counts, the panel
  draws.
- The headline is WITHHELD, not zeroed, when a source is down: panel-metrics
  omits `homelab_problems_total` entirely, the key goes stale, and this renders
  "--". A zero that means "blind" is the one lie this panel must never tell.
- Type is sized against the vendor boot logo (~159 display px, 42% of panel
  height), the only physical reference we have for this glass. The headline is
  ~0.8x the logo; tile values ~0.26x.
- `fontSize` is NOT pixels: rendered ink height is 0.76x the JSON number.
"""

import json
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

# Where DejaVuSans lives, in preference order: a fonts/ directory beside this
# script wins so a pinned copy can guarantee identical output, then the usual
# system locations. Phases 1 and 2 hardcoded the local directory and could only
# be regenerated on a machine that happened to have one.
FONT_SEARCH = [
    Path(__file__).parent / "fonts" / "DejaVuSans.ttf",
    Path("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
    Path("/usr/share/fonts/dejavu/DejaVuSans.ttf"),
    Path("/usr/share/fonts/TTF/DejaVuSans.ttf"),
    Path("/Library/Fonts/DejaVuSans.ttf"),
]


def find_font() -> Path:
    for candidate in FONT_SEARCH:
        if candidate.is_file():
            return candidate
    raise SystemExit(
        "DejaVuSans.ttf not found. Install it (Debian: fonts-dejavu-core) or "
        "drop a copy in " + str(FONT_SEARCH[0])
    )

W, H = 960, 376
BG = (13, 17, 23)
TILE = (22, 27, 34)
EDGE = (48, 54, 61)
NAME = (200, 209, 222)
DIM = (125, 136, 153)

# PIL wants tuples, the panel format wants #RRGGBB. Mixing them up serialises an
# array into fontColor and the config fails to parse.
NEUTRAL = "#c8d1de"
DIM_HEX = "#7d8899"
GOOD = "#7ee787"
WARN = "#ffb454"
BAD = "#ff6b6b"
# Base colour for every thresholded value. A reading that matches no band is
# unknown, and unknown must not inherit the colour of the best band — the
# lesson from Kuma reporting "-1 ms" in green as though it were the fastest
# service on the panel.
NO_READING = "#5a6472"

MARGIN = 14
HEAD_H = 38
BODY_Y = MARGIN + HEAD_H + 8       # 60
BODY_B = H - MARGIN                # 362

# Left column: the headline. Right column: the grid that explains it.
HERO_X, HERO_W = MARGIN, 322
RULE_X = HERO_X + HERO_W + 12
GRID_X = RULE_X + 12
GRID_W = W - MARGIN - GRID_X
COL_GAP, ROW_GAP = 12, 7
COL_W = (GRID_W - COL_GAP) // 2
ROW_H = (BODY_B - BODY_Y - 2 * ROW_GAP) // 3

# (label, kind, sensor keys...). "pair" renders "a / b"; "one" a single value.
#
# Sensor keys are the metric names panel-metrics emits. The aggregates need no
# entry in sensor-mapping.cfg because they carry no Prometheus labels — that is
# the point of aggregating upstream. The two labelled ones do; see the README.
TILES = [
    ("POOLS",     "pair", "truenas_pools_online", "truenas_pools_total"),
    ("MONITORS",  "pair", "kuma_monitors_up", "kuma_monitors_total"),
    ("CERT MIN",  "one",  "kuma_cert_days_min"),
    ("WAN GW",    "one",  "opnsense_wan_gateway"),
    ("NAS UP",    "one",  "truenas_uptime_days"),
    ("INTERNET",  "one",  "hass_speedtest_download"),
]
# Not shown: `truenas_load1`. The live reading is 0.0019, which renders as "0.00"
# — a tile that looks like a failed sensor while reporting perfect health. A NAS
# that idles has nothing to say about its load average, and a panel that shows it
# anyway is furniture. Uptime takes the slot instead: it catches the unplanned
# reboot, which is a thing you actually want a glance to reveal.


def tile_box(i):
    """Top-left of tile i, filling column by column down each row."""
    col, row = i % 2, i // 2
    return GRID_X + col * (COL_W + COL_GAP), BODY_Y + row * (ROW_H + ROW_GAP)


def build_background(path: Path, font_path: Path) -> None:
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    head_font = ImageFont.truetype(str(font_path), 24)
    label_font = ImageFont.truetype(str(font_path), 19)
    hero_font = ImageFont.truetype(str(font_path), 27)
    slash_font = ImageFont.truetype(str(font_path), 34)

    d.text((30, MARGIN + 4), "HOMELAB", font=head_font, fill=DIM)
    d.text((W - MARGIN - 70, MARGIN + 2), "/", font=head_font, fill=DIM)
    d.text((W - MARGIN - 250, MARGIN + 6), "SOURCES", font=head_font, fill=DIM)
    d.line([(MARGIN, MARGIN + HEAD_H), (W - MARGIN, MARGIN + HEAD_H)], fill=EDGE)

    # The headline's caption, centred under the number it names.
    bbox = d.textbbox((0, 0), "PROBLEMS", font=hero_font)
    d.text((HERO_X + (HERO_W - (bbox[2] - bbox[0])) / 2 - bbox[0], 278),
           "PROBLEMS", font=hero_font, fill=DIM)

    # Separates the alarm from the detail. Short of full height so it reads as a
    # divider rather than as a box edge.
    d.line([(RULE_X, BODY_Y + 18), (RULE_X, BODY_B - 18)], fill=EDGE)

    for i, spec in enumerate(TILES):
        x, y = tile_box(i)
        d.rectangle([x, y, x + COL_W, y + ROW_H], fill=TILE)
        d.text((x + 16, y + 11), spec[0], font=label_font, fill=DIM)
        if spec[1] == "pair":
            d.text((x + 116, y + 44), "/", font=slash_font, fill=DIM)

    img.save(path)


def sensor(label, x, y, w, h, size, colour, *, font="HarmonyOS_Sans_SC_Bold",
           align="left", unit="", decimals=0, thresholds=None, value_map=None):
    s = {
        "mode": 1, "type": 3, "itemName": label, "label": label,
        "x": x, "y": y, "width": w, "height": h,
        "textDirection": 0, "direction": 1, "value": "",
        "fontFamily": font, "fontSize": size, "fontColor": colour,
        "fontWeight": "normal", "textAlign": align,
        "integerDigits": -1, "decimalDigits": decimals, "unit": unit,
        "minAngle": 0, "maxAngle": 180, "minValue": 0, "maxValue": 100,
        "pic": "", "xz_x": 0, "xz_y": 0,
        "staleText": "--", "staleColor": NO_READING,
    }
    if thresholds:
        s["thresholds"] = [{"min": m, "color": c} for m, c in thresholds]
    if value_map:
        s["valueMap"] = value_map
    return s


# Per-tile rendering options, keyed by sensor. Anything absent renders neutral,
# which is the default on purpose: colour belongs to the headline.
STYLE = {
    # Note the top band is NEUTRAL, not GOOD. A healthy value has nothing to
    # announce, and a tile that turns green for being fine competes with the
    # headline for the one glance you give this panel. Only the bands that mean
    # "act on me" get colour.
    "kuma_cert_days_min": dict(
        unit=" d",
        thresholds=[(0, BAD), (7, WARN), (21, NEUTRAL)],
    ),
    "opnsense_wan_gateway": dict(
        thresholds=[(0, BAD), (1, NEUTRAL)],
        value_map={"0": "DOWN", "1": "ONLINE"},
    ),
    # One decimal, so a reboot is visible within a couple of hours rather than
    # after a whole day of the tile reading "0".
    "truenas_uptime_days": dict(decimals=1, unit=" d"),
    "hass_speedtest_download": dict(unit=" Mb"),
}


def build_panel() -> dict:
    s = [
        # The headline. Grey base so a value outside every band reads as unknown
        # rather than as healthy.
        sensor("homelab_problems_total", HERO_X, 118, HERO_W, 156, 170, NO_READING,
               align="center",
               thresholds=[(0, GOOD), (1, WARN), (3, BAD)]),
        # Sensor-pipeline health, so a dead scraper is distinguishable from a
        # quiet homelab. Counts sensor files, not panel-metrics sources.
        sensor("SYS_sources_live", W - MARGIN - 148, MARGIN + 2, 60, 32, 34,
               "#a684ff", align="right"),
        sensor("SYS_sources_total", W - MARGIN - 56, MARGIN + 2, 50, 32, 34,
               "#7d8899", align="left"),
    ]

    for i, spec in enumerate(TILES):
        x, y = tile_box(i)
        if spec[1] == "pair":
            s.append(sensor(spec[2], x + 14, y + 40, 94, 44, 54, NEUTRAL,
                            align="right", **STYLE.get(spec[2], {})))
            # The denominator is dimmer than the numerator: "5" is the reading,
            # "/ 5" is context, and equal weight makes the pair read as one
            # four-digit number at a glance.
            s.append(sensor(spec[3], x + 132, y + 40, 94, 44, 54, DIM_HEX,
                            align="left", **STYLE.get(spec[3], {})))
        else:
            s.append(sensor(spec[2], x + 14, y + 40, COL_W - 28, 44, 54, NEUTRAL,
                            align="left", **STYLE.get(spec[2], {})))

    return {"id": "phase3-homelab", "name": "phase3-homelab",
            "img": "bg.png", "sensor": s}


def main() -> None:
    here = Path(__file__).parent
    (here / "img").mkdir(exist_ok=True)
    build_background(here / "img" / "bg.png", find_font())
    panel = build_panel()
    (here / "panel.json").write_text(json.dumps(panel, indent=2) + "\n")
    print(f"{len(TILES)} tiles, {len(panel['sensor'])} sensors")


if __name__ == "__main__":
    main()
