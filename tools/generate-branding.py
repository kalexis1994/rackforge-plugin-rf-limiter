#!/usr/bin/env python3
"""Generate the original RF-Limiter branding assets RackForge requires.

RackForge validates three PNGs by exact size: a 512x512 icon, a 1600x400
banner and a 1920x1080 splash. They are drawn here rather than committed as
opaque binaries so the visual identity is reviewable, reproducible, and
unmistakably this project's own work: a waveform, a ceiling, and the peaks
that stopped at it.

Run:  python tools/generate-branding.py
"""

from __future__ import annotations

import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[1]
OUTPUT = ROOT / "plugin" / "package" / "branding"

FONT_CANDIDATES = (
    Path("C:/Windows/Fonts/arialbd.ttf"),
    Path("C:/Windows/Fonts/segoeuib.ttf"),
    Path("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"),
)

# The palette also lives in rackforge-plugin.toml; keep the two in step.
PANEL = (14, 18, 22)
PANEL_LIGHT = (27, 35, 43)
INK = (232, 236, 239)
MUTED = (139, 150, 160)
ACCENT = (228, 85, 63)
ACCENT_DIM = (107, 42, 32)
STEEL = (174, 182, 189)
WAVE = (88, 132, 160)


def font(size: int) -> ImageFont.FreeTypeFont:
    for candidate in FONT_CANDIDATES:
        if candidate.exists():
            return ImageFont.truetype(str(candidate), size)
    return ImageFont.load_default()


def field(size: tuple[int, int]) -> Image.Image:
    """The dark brushed field everything sits on."""
    width, height = size
    image = Image.new("RGB", size, PANEL)
    draw = ImageDraw.Draw(image)
    step = max(6, height // 90)
    for y in range(0, height, step):
        shade = 4 if (y // step) % 2 == 0 else 0
        draw.line([(0, y), (width, y)], fill=(PANEL[0] + shade, PANEL[1] + shade, PANEL[2] + shade))
    return image


def waveform(x: float, seed: int = 3) -> float:
    """A signal with peaks: a chord of sines whose crests line up now and then."""
    return (
        0.55 * math.sin(x * 3.1 + seed)
        + 0.30 * math.sin(x * 7.3 + 1.2)
        + 0.25 * math.sin(x * 12.7 + 0.4)
        + 0.18 * math.sin(x * 19.1 + 2.2)
    )


def limited_wave(
    draw: ImageDraw.ImageDraw,
    box: tuple[float, float, float, float],
    ceiling: float,
    stroke: int,
) -> None:
    """The waveform across `box`, with everything above `ceiling` held down and
    the ceiling drawn as the line it is."""
    left, top, right, bottom = box
    middle = (top + bottom) / 2
    half = (bottom - top) / 2
    points_raw = []
    points_held = []
    steps = int(right - left)
    for i in range(steps + 1):
        x = left + i
        t = (i / max(1, steps)) * 6.0
        value = waveform(t)
        points_raw.append((x, middle - value * half))
        held = max(-ceiling, min(ceiling, value))
        points_held.append((x, middle - held * half))
    draw.line(points_raw, fill=ACCENT_DIM, width=max(1, stroke // 2))
    draw.line(points_held, fill=WAVE, width=stroke)
    for sign in (1, -1):
        y = middle - sign * ceiling * half
        dash = 14
        x = left
        while x < right:
            draw.line([(x, y), (min(right, x + dash), y)], fill=ACCENT, width=max(2, stroke // 2))
            x += dash * 2


def make_icon() -> None:
    size = (512, 512)
    image = field(size)
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle([28, 28, 484, 484], radius=72, fill=PANEL_LIGHT, outline=STEEL, width=6)
    limited_wave(draw, (70, 150, 442, 362), ceiling=0.62, stroke=10)
    label = font(64)
    draw.text((256, 428), "LIM", font=label, fill=INK, anchor="mm")
    image.save(OUTPUT / "icon.png")


def make_banner() -> None:
    size = (1600, 400)
    image = field(size)
    draw = ImageDraw.Draw(image)
    limited_wave(draw, (60, 70, 900, 330), ceiling=0.6, stroke=8)
    title = font(96)
    draw.text((1240, 160), "RF-LIMITER", font=title, fill=INK, anchor="mm")
    subtitle = font(34)
    draw.text((1240, 240), "True peak · lookahead · final protection", font=subtitle, fill=ACCENT, anchor="mm")
    image.save(OUTPUT / "banner.png")


def make_splash() -> None:
    size = (1920, 1080)
    image = field(size)
    draw = ImageDraw.Draw(image)
    title = font(150)
    draw.text((960, 200), "RF-LIMITER", font=title, fill=INK, anchor="mm")
    subtitle = font(44)
    draw.text((960, 320), "True peak · lookahead · final protection", font=subtitle, fill=ACCENT, anchor="mm")
    draw.rounded_rectangle([160, 420, 1760, 920], radius=40, fill=PANEL_LIGHT, outline=STEEL, width=4)
    limited_wave(draw, (220, 470, 1700, 870), ceiling=0.6, stroke=10)
    footer = font(34)
    draw.text(
        (960, 990),
        "guaranteed final ceiling  ·  programme release  ·  delta listen  ·  live level and reduction history",
        font=footer,
        fill=MUTED,
        anchor="mm",
    )
    image.save(OUTPUT / "splash.png")


def main() -> None:
    OUTPUT.mkdir(parents=True, exist_ok=True)
    make_icon()
    make_banner()
    make_splash()
    for name, expected in (("icon.png", (512, 512)), ("banner.png", (1600, 400)), ("splash.png", (1920, 1080))):
        with Image.open(OUTPUT / name) as image:
            assert image.size == expected, f"{name} is {image.size}, expected {expected}"
            assert image.mode in ("RGB", "RGBA"), f"{name} is {image.mode}"
        print(f"wrote {OUTPUT / name}")


if __name__ == "__main__":
    main()
