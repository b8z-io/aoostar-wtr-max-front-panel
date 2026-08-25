#!/usr/bin/env python3
"""Generate the Phase 1 host panel: background artwork + panel.json.

Layout targets a 960x376 RGB565 LCD. Design notes:

- Regions are wide-and-short where possible. Chunks are 47 bytes = 23.5px of a
  row-major buffer, so a full-width band is cheap to repaint and a narrow column
  pays a whole chunk per row. See ops/ARCHITECTURE.md section 5.
- fontSize is NOT pixels: rendered ink height is 0.76x the JSON number.
- Every value carries staleText/staleColor so a dead provider reads as absent
  rather than as a plausible number.
"""
import json
import pathlib
from PIL import Image, ImageDraw, ImageFont

W, H = 960, 376
BG        = (13, 17, 23)
TILE      = (22, 27, 34)
TILE_EDGE = (48, 54, 61)
DIM       = "#7d8590"
FG        = "#e6edf3"
STALE     = "#6e7681"

ACCENT_CPU  = "#ff9e64"
ACCENT_MEM  = "#7aa2f7"
ACCENT_DISK = "#9ece6a"
ACCENT_NET  = "#bb9af7"

MARGIN, GAP = 16, 16
COL_W  = (W - MARGIN * 2 - GAP * 2) // 3
TOP_Y, TOP_H = MARGIN, 240
BAR_Y, BAR_H = TOP_Y + TOP_H + GAP, H - (MARGIN + TOP_Y + TOP_H + GAP)
COLS = [MARGIN + i * (COL_W + GAP) for i in range(3)]


def tile(d, x, y, w, h, accent=None):
    d.rounded_rectangle([x, y, x + w, y + h], radius=10, fill=TILE, outline=TILE_EDGE)
    if accent:
        d.rounded_rectangle([x, y, x + w, y + 4], radius=2, fill=accent)


def build_background(path):
    img = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(img)
    tile(d, COLS[0], TOP_Y, COL_W, TOP_H, ACCENT_CPU)
    tile(d, COLS[1], TOP_Y, COL_W, TOP_H, ACCENT_MEM)
    tile(d, COLS[2], TOP_Y, COL_W, TOP_H, ACCENT_DISK)
    # divider inside the middle and right columns, separating their two readings
    for c in (COLS[1], COLS[2]):
        d.line([(c + 20, TOP_Y + 132), (c + COL_W - 20, TOP_Y + 132)], fill=TILE_EDGE)
    tile(d, MARGIN, BAR_Y, W - MARGIN * 2, BAR_H, ACCENT_NET)

    font_dir = pathlib.Path(__file__).parent / "fonts"
    cap_font = ImageFont.truetype(str(font_dir / "DejaVuSans.ttf"), 15)
    for text, x, y, w in CAPTIONS:
        tw = d.textlength(text, font=cap_font)
        d.text((x + (w - tw) / 2, y), text, font=cap_font, fill=(125, 133, 144))
    d.text((W - MARGIN - 300, BAR_Y + 14), "SOURCES LIVE", font=cap_font,
           fill=(125, 133, 144))
    d.text((W - MARGIN - 166, BAR_Y + 48), "/", font=cap_font, fill=(90, 98, 108))
    img.save(path)


def sensor(label, x, y, w, h, size, colour, font="HarmonyOS_Sans_SC_Bold",
           align="center", unit="", decimals=-1, stale="--"):
    return {
        "mode": 1, "type": 3, "itemName": label, "label": label,
        "x": x, "y": y, "width": w, "height": h,
        "textDirection": 0, "direction": 1, "value": "",
        "fontFamily": font, "fontSize": size, "fontColor": colour,
        "fontWeight": "normal", "textAlign": align,
        "integerDigits": -1, "decimalDigits": decimals,
        "unit": unit, "minAngle": 0, "maxAngle": 180,
        "minValue": 0, "maxValue": 100, "pic": "", "xz_x": 0, "xz_y": 0,
        "staleText": stale, "staleColor": STALE,
    }


# Captions are painted into the background, not driven as sensors. They are
# artwork, not readings: a label fed through the sensor file would go stale
# along with everything else and vanish exactly when the panel most needs to
# stay readable. Painting them also keeps them out of the chunk budget, since
# the background is cached and never retransmitted.
CAPTIONS = [
    ("CPU",          COLS[0], TOP_Y + 18, COL_W),
    ("PACKAGE TEMP", COLS[0], TOP_Y + 154, COL_W),
    ("MEMORY",       COLS[1], TOP_Y + 18, COL_W),
    ("LOAD 1M",      COLS[1], TOP_Y + 150, COL_W),
    ("ROOT DISK",    COLS[2], TOP_Y + 18, COL_W),
    ("UPTIME",       COLS[2], TOP_Y + 150, COL_W),
]


s = []
# --- column 1: CPU -------------------------------------------------------
s.append(sensor("cpu_usage_percent", COLS[0], TOP_Y + 48, COL_W, 96, 76, FG,
                unit="%", decimals=0))
s.append(sensor("temperature_cpu", COLS[0], TOP_Y + 180, COL_W, 48, 40, ACCENT_CPU,
                unit=" °C", decimals=0))

# --- column 2: memory over load -----------------------------------------
s.append(sensor("mem_usage_percent", COLS[1], TOP_Y + 46, COL_W, 76, 56, FG,
                unit="%", decimals=0))
s.append(sensor("load_avg_one", COLS[1], TOP_Y + 178, COL_W, 52, 44, ACCENT_MEM,
                decimals=2))

# --- column 3: disk over uptime -----------------------------------------
s.append(sensor("disk_root_usage_percent", COLS[2], TOP_Y + 46, COL_W, 76, 56, FG,
                unit="%", decimals=0))
s.append(sensor("system_uptime", COLS[2], TOP_Y + 178, COL_W, 52, 40, ACCENT_DISK))

# --- bottom bar: clock, host, source health ------------------------------
s.append(sensor("DATE_h_m_s_1", MARGIN + 24, BAR_Y + 12, 260, BAR_H - 24, 46, FG,
                align="left"))
s.append(sensor("DATE_y_m_d_2", MARGIN + 300, BAR_Y + 14, 200, 30, 20, DIM,
                font="DejaVuSans", align="left"))
s.append(sensor("system_hostname", MARGIN + 300, BAR_Y + 46, 200, 28, 20, DIM,
                font="DejaVuSans", align="left"))
# Source health is computed at render time, so it stays truthful when every
# provider is dead. This is the element that says "the renderer is alive".
s.append(sensor("SYS_sources_live", W - MARGIN - 300, BAR_Y + 38, 130, 40, 34,
                ACCENT_NET, align="right"))
s.append(sensor("SYS_sources_total", W - MARGIN - 140, BAR_Y + 38, 116, 40, 34,
                DIM, align="left"))

panel = {"id": "phase1-host", "name": "phase1-host", "img": "bg.png", "sensor": s}

if __name__ == "__main__":
    import pathlib
    here = pathlib.Path(__file__).parent
    build_background(here / "img" / "bg.png")
    (here / "panel.json").write_text(json.dumps(panel, indent=2) + "\n")
    print(f"{len(s)} sensors written")
