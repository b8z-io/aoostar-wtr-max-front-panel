#!/usr/bin/env python3
"""Generate the Phase 2 services panel: background artwork plus panel.json.

Shows five Uptime-Kuma monitors as name / state / response time.

Design notes
------------
- Monitor names are static, so they are painted into the background rather than
  spending a sensor on each. Sensors are for things that change.
- Kuma reports state as 1 or 0. `valueMap` turns that into UP / DOWN, and
  `thresholds` colour it from the raw number — so DOWN is red even though the
  text says DOWN rather than 0.
- Type is sized against the vendor boot logo (~159 display px, 42% of panel
  height), the only physical reference we have for this glass. Values sit at
  ~0.3x the logo, names at ~0.2x.
- `fontSize` is NOT pixels: rendered ink height is 0.76x the JSON number.
- No uptime percentage. Three columns read far better than four at a glance,
  and a 30-day ratio is a number you look up, not one you glance at.
"""

import json
from pathlib import Path
from PIL import Image, ImageDraw, ImageFont

W, H = 960, 376
BG = (13, 17, 23)
ROW_A = (22, 27, 34)
ROW_B = (17, 21, 27)
EDGE = (48, 54, 61)
NAME = (200, 209, 222)
DIM = (125, 136, 153)
# Same grey as DIM, but as a panel-JSON colour string. PIL wants a tuple and the
# panel format wants #RRGGBB; mixing them up serialises an array into fontColor
# and the config fails to parse.
DIM_HEX = "#7d8899"

# Colour bands. "No reading" is grey (staleColor) and must not be confused with
# either state, so neither band uses grey.
UP = "#7ee787"
DOWN = "#ff6b6b"
FAST = "#7ee787"
OK = "#ffb454"
SLOW = "#ff6b6b"
# Kuma reports 0 ms for a monitor that is down. Left alone that renders as the
# fastest service on the panel, in green — actively misleading next to a red
# DOWN. Zero gets its own grey band and reads as "no measurement", matching the
# staleness placeholder.
NO_READING = "#5a6472"

MARGIN = 14
HEAD_H = 38
ROW_H = 62
ROWS_Y = MARGIN + HEAD_H + 6

# Kuma monitors, in the order they appear. Panel labels are stable names; the
# real provider keys carry the full Prometheus label set and are resolved by
# cfg/sensor-mapping.cfg on the host.
MONITORS = [
    ("INTERNET", "internet"),
    ("OPNSENSE", "opnsense"),
    ("TRAEFIK", "traefik"),
    ("HINDSIGHT", "hindsight"),
    ("CLOUDFLARE", "cloudflare"),
]

COL_NAME_X = 30
COL_STATE_X = 366
COL_RESP_X = 600
COL_RESP_W = W - MARGIN - COL_RESP_X - 16


def build_background(path: Path, font_dir: Path) -> None:
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    name_font = ImageFont.truetype(str(font_dir / "DejaVuSans.ttf"), 30)
    head_font = ImageFont.truetype(str(font_dir / "DejaVuSans.ttf"), 24)

    d.text((COL_NAME_X, MARGIN + 4), "SERVICES", font=head_font, fill=DIM)
    d.text((W - MARGIN - 70, MARGIN + 2), "/", font=head_font, fill=DIM)
    d.text((W - MARGIN - 250, MARGIN + 6), "SOURCES", font=head_font, fill=DIM)
    d.line([(MARGIN, MARGIN + HEAD_H), (W - MARGIN, MARGIN + HEAD_H)], fill=EDGE)

    for i, (display_name, _) in enumerate(MONITORS):
        y = ROWS_Y + i * ROW_H
        d.rectangle([MARGIN, y, W - MARGIN, y + ROW_H - 4],
                    fill=ROW_A if i % 2 == 0 else ROW_B)
        # vertically centre the name against the row
        bbox = d.textbbox((0, 0), display_name, font=name_font)
        d.text((COL_NAME_X, y + (ROW_H - 4 - (bbox[3] - bbox[1])) / 2 - bbox[1]),
               display_name, font=name_font, fill=NAME)

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
        "staleText": "--", "staleColor": "#5a6472",
    }
    if thresholds:
        s["thresholds"] = [{"min": m, "color": c} for m, c in thresholds]
    if value_map:
        s["valueMap"] = value_map
    return s


def build_panel() -> dict:
    s = []
    for i, (_, key) in enumerate(MONITORS):
        y = ROWS_Y + i * ROW_H
        s.append(sensor(
            f"kuma_{key}_status", COL_STATE_X, y + 6, 190, ROW_H - 16, 46, DOWN,
            thresholds=[(0, DOWN), (1, UP)],
            value_map={"0": "DOWN", "1": "UP"},
        ))
        s.append(sensor(
            f"kuma_{key}_response", COL_RESP_X, y + 6, COL_RESP_W, ROW_H - 16, 44, FAST,
            align="right", unit=" ms",
            thresholds=[(0, NO_READING), (1, FAST), (200, OK), (500, SLOW)],
            value_map={"0": "--"},
        ))

    # Source health, so a dead scrape is distinguishable from every monitor being up.
    s.append(sensor("SYS_sources_live", W - MARGIN - 148, MARGIN + 2, 60, 32, 34,
                    "#a684ff", align="right"))
    s.append(sensor("SYS_sources_total", W - MARGIN - 56, MARGIN + 2, 50, 32, 34,
                    DIM_HEX, align="left"))

    return {"id": "phase2-services", "name": "phase2-services",
            "img": "bg.png", "sensor": s}


def main() -> None:
    here = Path(__file__).parent
    build_background(here / "img" / "bg.png", here / "fonts")
    (here / "panel.json").write_text(json.dumps(build_panel(), indent=2) + "\n")
    print(f"{len(MONITORS)} monitors, {len(build_panel()['sensor'])} sensors")


if __name__ == "__main__":
    main()
