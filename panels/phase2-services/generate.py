#!/usr/bin/env python3
"""Generate the Phase 2 services panel: background artwork plus panel.json.

Ten Uptime-Kuma monitors as name / state. Two columns of five.

Design notes
------------
- NO RESPONSE-TIME COLUMN. There used to be one, and it was a mistake. Three of
  the five monitors on the old panel are Kuma `docker` type, which reports
  up/down and no latency at all — so 60% of that column was a permanent "--"
  occupying a third of the panel width. The fix is not to pick monitors that
  happen to report latency (that is the tail wagging the dog); it is to drop the
  column and spend the width on twice as many services in much larger type.
- THE COLUMNS MEAN SOMETHING. Left is the path in — firewall, proxy, internet,
  tunnel, VPN. Right is the quiet stuff, where failure is silent. Reading down a
  column tells you which kind of thing is broken.
- SELECTED FOR SILENT FAILURE, NOT POPULARITY. A glance panel earns its place on
  faults you would not otherwise notice. Jellyfin being down announces itself the
  moment you try to watch something; a notification relay that died three days
  ago does not. Hence APPRISE (if it is down, Kuma sees every problem and can
  tell you about none of them) and GLUETUN (a dropped VPN leaks the home IP while
  every service keeps working perfectly).
- SPREAD ACROSS HOSTS. Eight of these run on docker2, so a wedged docker2 turns
  most of the panel red at once and tells you one fact. OPNSENSE and HAOS are on
  other machines, which is what makes those two rows worth their space — they
  distinguish "docker2 is unwell" from "the homelab is unwell".
- UP IS MUTED, DOWN IS RED. Nine green rows shout as loudly as the one that
  matters, and within a week you stop reading the panel — the same failure the
  Phase 3 headline was designed against. Colour belongs to the exception.
- Monitor names are static, so they are painted into the background rather than
  spending a sensor on each. Sensors are for things that change.
- Type is sized against the vendor boot logo (~159 display px, 42% of panel
  height). `fontSize` is NOT pixels: rendered ink height is 0.76x the JSON number.
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
DIM_HEX = "#7d8899"

# UP is deliberately not green. See the design note above.
UP = "#8b96a5"
DOWN = "#ff6b6b"
# Grey is the BASE colour, so a value matching no band reads as "unknown" rather
# than as "fine" — the lesson from Kuma's -1 latency sentinel rendering green.
NO_READING = "#5a6472"

MARGIN = 14
HEAD_H = 38
ROW_H = 60
ROWS_Y = MARGIN + HEAD_H + 8
COL_GAP = 16
COL_W = (W - 2 * MARGIN - COL_GAP) // 2

# (display name, sensor key). Keys are stable names the panel asks for; the real
# provider keys carry the full Prometheus label set and are resolved per host by
# cfg/sensor-mapping.cfg.
#
# Display names are deliberately not always the raw monitor names. Kuma has both
# "cloudflare" (a 1.1.1.1 ping) and "cloudflared" (the tunnel container) — one
# character apart, adjacent on the panel, and meaning completely different
# things. Named for what they tell you instead.
#
# First five: the path in, ordered outward from the firewall.
# Second five: silent failures, ordered by how fast you would want to act.
MONITORS = [
    ("OPNSENSE", "opnsense"),    # firewall — separate box
    ("TRAEFIK", "traefik"),      # reverse proxy
    ("INTERNET", "cloudflare"),  # 1.1.1.1 ping — reachability canary
    ("TUNNEL", "cloudflared"),   # the way back in
    ("GLUETUN", "gluetun"),      # VPN — a drop leaks the home IP silently
    ("HAOS", "haos"),            # Home Assistant — separate VM
    ("APPRISE", "apprise"),      # notification relay — down means silence
    ("DOCKHAND", "dockhand"),    # Docker API the automation depends on
    ("N8N", "n8n"),              # workflow engine
    ("DIUN", "diun"),            # update notifier — stale containers, no warning
]

NAME_X = 22
STATE_W = 170


def cell(i):
    """Top-left of row i. Fills column by column, five per column."""
    col, row = i // 5, i % 5
    return MARGIN + col * (COL_W + COL_GAP), ROWS_Y + row * ROW_H


def find_font(here: Path) -> Path:
    """DejaVuSans, from the panel's fonts symlink or the usual system locations."""
    for candidate in [
        here / "fonts" / "DejaVuSans.ttf",
        Path("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"),
        Path("/usr/share/fonts/dejavu/DejaVuSans.ttf"),
        Path("/usr/share/fonts/TTF/DejaVuSans.ttf"),
    ]:
        if candidate.is_file():
            return candidate
    raise SystemExit("DejaVuSans.ttf not found (Debian: fonts-dejavu-core)")


def build_background(path: Path, font_path: Path) -> None:
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    name_font = ImageFont.truetype(str(font_path), 32)
    head_font = ImageFont.truetype(str(font_path), 24)

    d.text((30, MARGIN + 4), "SERVICES", font=head_font, fill=DIM)
    d.text((W - MARGIN - 70, MARGIN + 2), "/", font=head_font, fill=DIM)
    d.text((W - MARGIN - 250, MARGIN + 6), "SOURCES", font=head_font, fill=DIM)
    d.line([(MARGIN, MARGIN + HEAD_H), (W - MARGIN, MARGIN + HEAD_H)], fill=EDGE)

    for i, (display_name, _) in enumerate(MONITORS):
        x, y = cell(i)
        d.rectangle([x, y, x + COL_W, y + ROW_H - 5],
                    fill=ROW_A if i % 2 == 0 else ROW_B)
        bbox = d.textbbox((0, 0), display_name, font=name_font)
        d.text((x + NAME_X, y + (ROW_H - 5 - (bbox[3] - bbox[1])) / 2 - bbox[1]),
               display_name, font=name_font, fill=NAME)

    img.save(path)


def sensor(label, x, y, w, h, size, colour, *, font="HarmonyOS_Sans_SC_Bold",
           align="left", thresholds=None, value_map=None):
    s = {
        "mode": 1, "type": 3, "itemName": label, "label": label,
        "x": x, "y": y, "width": w, "height": h,
        "textDirection": 0, "direction": 1, "value": "",
        "fontFamily": font, "fontSize": size, "fontColor": colour,
        "fontWeight": "normal", "textAlign": align,
        "integerDigits": -1, "decimalDigits": 0, "unit": "",
        "minAngle": 0, "maxAngle": 180, "minValue": 0, "maxValue": 100,
        "pic": "", "xz_x": 0, "xz_y": 0,
        "staleText": "--", "staleColor": NO_READING,
    }
    if thresholds:
        s["thresholds"] = [{"min": m, "color": c} for m, c in thresholds]
    if value_map:
        s["valueMap"] = value_map
    return s


def build_panel() -> dict:
    s = []
    for i, (_, key) in enumerate(MONITORS):
        x, y = cell(i)
        s.append(sensor(
            f"kuma_{key}_status", x + COL_W - STATE_W - 20, y + 6, STATE_W, ROW_H - 18,
            48, DOWN, align="right",
            thresholds=[(0, DOWN), (1, UP)],
            value_map={"0": "DOWN", "1": "UP"},
        ))

    # Source health, so a dead scrape is distinguishable from everything being up.
    s.append(sensor("SYS_sources_live", W - MARGIN - 148, MARGIN + 2, 60, 32, 34,
                    "#a684ff", align="right"))
    s.append(sensor("SYS_sources_total", W - MARGIN - 56, MARGIN + 2, 50, 32, 34,
                    DIM_HEX, align="left"))

    return {"id": "phase2-services", "name": "phase2-services",
            "img": "bg.png", "sensor": s}


def main() -> None:
    here = Path(__file__).parent
    (here / "img").mkdir(exist_ok=True)
    build_background(here / "img" / "bg.png", find_font(here))
    panel = build_panel()
    (here / "panel.json").write_text(json.dumps(panel, indent=2) + "\n")
    print(f"{len(MONITORS)} monitors, {len(panel['sensor'])} sensors")


if __name__ == "__main__":
    main()
