#!/usr/bin/env python3
"""Малює значок програми й збирає assets/icon.ico (працює) та assets/icon_off.ico (пауза).

Стилі (перший аргумент):
  dual (типово) — дві накладені клавіатури на темній плитці: сіро-синя позаду, салатова попереду (дві розкладки).
                  Великі розміри беруться з готового малюнка assets/icon_dual_source.png, а для 16–32 px
                  малюється спрощений варіант (менше клавіш);
  lime          — салатова клавіатура на темній плитці, над клавішею спливає біла «підказка» з літерою «Ї»;
  neon          — темна плитка з неоновим сяйвом: дві клавіші «A» і «Я» всередині стрілок-«перемикача»;
  flag          — синьо-жовта клавіша з діагоналлю («A» і дзеркальна «Я»).
У режимі паузи значок сірий, з двома смужками «пауза».

Потрібні Pillow, numpy і шрифт DejaVu Sans Bold (є в більшості Linux). Запуск із кореня проєкту:
    python3 tools/make_icon.py [dual|lime|neon|flag] [тека-для-результату]
Результат (типово в assets/) — icon.ico, icon_off.ico і preview.png (для перегляду, у збірку не йде).
"""
import math
import struct
import sys
from io import BytesIO
from pathlib import Path

import numpy as np
from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont, ImageOps

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "assets"
FONT = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"

BIG = 1024  # малюємо у великому розмірі й стискаємо: так краї гладкі
SIZES = [16, 20, 24, 32, 40, 48, 64, 256]

BLUE_TOP, BLUE_BOTTOM = (38, 118, 232), (10, 72, 170)
GOLD_TOP, GOLD_BOTTOM = (255, 214, 64), (244, 170, 0)
NAVY = (8, 38, 92)
WHITE = (255, 255, 255)


def vgradient(size, top, bottom):
    """Вертикальний градієнт RGBA."""
    col = Image.new("RGB", (1, size))
    px = col.load()
    for y in range(size):
        t = y / (size - 1)
        px[0, y] = tuple(round(top[i] + (bottom[i] - top[i]) * t) for i in range(3))
    return col.resize((size, size)).convert("RGBA")


def rounded_mask(size, margin, radius):
    m = Image.new("L", (size, size), 0)
    ImageDraw.Draw(m).rounded_rectangle([margin, margin, size - margin - 1, size - margin - 1], radius, fill=255)
    return m


def base_art(size=BIG, small=False):
    """Кольорове зображення без паузи. `small` — спрощений варіант для 16–32 px: крупніші літери, без тіні."""
    margin, radius = int(size * 0.045), int(size * 0.215)
    body = rounded_mask(size, margin, radius)

    # діагональ від правого верхнього кута до лівого нижнього
    tri_blue = Image.new("L", (size, size), 0)
    ImageDraw.Draw(tri_blue).polygon([(0, 0), (size, 0), (0, size)], fill=255)
    tri_gold = ImageChops.invert(tri_blue)

    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    img.paste(vgradient(size, BLUE_TOP, BLUE_BOTTOM), (0, 0), ImageChops.multiply(body, tri_blue))
    img.paste(vgradient(size, GOLD_TOP, GOLD_BOTTOM), (0, 0), ImageChops.multiply(body, tri_gold))

    d = ImageDraw.Draw(img)
    # світла лінія розділу
    w = int(size * (0.05 if small else 0.035))
    line = Image.new("L", (size, size), 0)
    ImageDraw.Draw(line).line([(size, 0), (0, size)], fill=255, width=w)
    img.paste(Image.new("RGBA", (size, size), WHITE + (235,)), (0, 0), ImageChops.multiply(line, body))

    # літери: «A» на синьому, «Я» на жовтому
    font = ImageFont.truetype(FONT, int(size * (0.58 if small else 0.46)))
    shadow = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    sd = ImageDraw.Draw(shadow)
    pos_a, pos_ya = ((size * 0.31, size * 0.36), (size * 0.69, size * 0.67)) if small else ((size * 0.335, size * 0.375), (size * 0.665, size * 0.665))
    for pos, txt in ((pos_a, "A"), (pos_ya, "Я")):
        sd.text((pos[0] + size * 0.012, pos[1] + size * 0.018), txt, font=font, fill=(0, 0, 0, 90), anchor="mm")
    shadow = shadow.filter(ImageFilter.GaussianBlur(size * 0.008))
    if not small:
        img = Image.alpha_composite(img, shadow)
    d = ImageDraw.Draw(img)
    d.text(pos_a, "A", font=font, fill=WHITE, anchor="mm")
    d.text(pos_ya, "Я", font=font, fill=NAVY, anchor="mm")

    # тонкий темний контур, щоб значок не розчинявся на світлій панелі
    edge = ImageChops.subtract(body, body.filter(ImageFilter.MinFilter(max(3, int(size * 0.012)) | 1)))
    img.paste(Image.new("RGBA", (size, size), (0, 25, 70, 120)), (0, 0), edge)
    return img


def paused_art(size=BIG, small=False):
    """Сіра версія з двома смужками «пауза» в правому нижньому куті."""
    color = base_art(size, small)
    gray = color.convert("LA").convert("RGBA")
    # трохи світліше й менш контрастно, щоб було видно: «не працює»
    gray = Image.blend(gray, Image.new("RGBA", (size, size), (200, 205, 212, 0)), 0.0)
    a = color.getchannel("A")
    gray.putalpha(a)
    gray = Image.eval(gray.convert("RGB"), lambda v: int(v * 0.62 + 70)).convert("RGBA")
    gray.putalpha(a)

    # плашка з паузою
    d = ImageDraw.Draw(gray)
    r = int(size * (0.27 if small else 0.2))
    cx, cy = int(size * (0.70 if small else 0.75)), int(size * (0.70 if small else 0.75))
    d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=(52, 58, 70, 255), outline=WHITE + (255,), width=int(size * 0.018))
    bw, bh, gap = int(size * (0.07 if small else 0.05)), int(size * (0.26 if small else 0.19)), int(size * (0.06 if small else 0.042))
    for dx in (-(gap // 2 + bw), gap // 2):
        d.rounded_rectangle([cx + dx, cy - bh // 2, cx + dx + bw, cy + bh // 2], int(size * 0.012), fill=WHITE)
    return gray


# ───────────────────────────── стиль «neon» ─────────────────────────────

BG_A, BG_B = (5, 7, 28), (9, 24, 78)           # темна плитка: від лівого верхнього до правого нижнього кута
CYAN, BLUE = (60, 245, 255), (35, 120, 255)
VIOLET, MAGENTA = (190, 100, 255), (255, 70, 200)


def lerp(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(3))


def diag_gradient(size, stops, angle=45.0):
    """Градієнт RGB уздовж напрямку `angle` (градуси, 0 = зліва направо, 90 = згори вниз). stops: [(t, (r,g,b)), ...]."""
    ys, xs = np.mgrid[0:size, 0:size].astype(np.float32)
    a = math.radians(angle)
    t = ((xs - size / 2) * math.cos(a) + (ys - size / 2) * math.sin(a)) / (size * (abs(math.cos(a)) + abs(math.sin(a)))) + 0.5
    t = np.clip(t, 0, 1)
    ts = [p[0] for p in stops]
    out = np.zeros((size, size, 3), np.float32)
    for c in range(3):
        out[..., c] = np.interp(t, ts, [p[1][c] for p in stops])
    return Image.fromarray(out.astype(np.uint8), "RGB")


def radial_glow(size, center, radius, color, strength):
    """Світла пляма з м'яким спадом (як окремий шар RGB, чорний поза плямою)."""
    ys, xs = np.mgrid[0:size, 0:size].astype(np.float32)
    d = np.sqrt((xs - center[0] * size) ** 2 + (ys - center[1] * size) ** 2) / (radius * size)
    k = np.clip(1 - d, 0, 1) ** 2 * strength
    out = np.zeros((size, size, 3), np.float32)
    for c in range(3):
        out[..., c] = color[c] * k
    return Image.fromarray(np.clip(out, 0, 255).astype(np.uint8), "RGB")


def screen_glow(base, shape_rgb, radii):
    """Додає до `base` сяйво від кольорового шару: кілька розмитих копій режимом screen."""
    out = base
    for r, k in radii:
        g = shape_rgb.filter(ImageFilter.GaussianBlur(r))
        if k != 1.0:
            g = g.point(lambda v, k=k: min(255, int(v * k)))
        out = ImageChops.screen(out, g)
    return out


def arc_ribbon(size, center, radius, a0, a1, w0, w1, head):
    """Маска вигнутої стрілки: тонкий хвіст (w0) розширюється до голови (w1), на кінці — трикутник."""
    cx, cy = center[0] * size, center[1] * size
    R = radius * size
    n = 140
    left, right = [], []
    for i in range(n + 1):
        t = i / n
        a = math.radians(a0 + (a1 - a0) * t)
        w = (w0 + (w1 - w0) * (t ** 1.3)) * size / 2
        ux, uy = math.cos(a), math.sin(a)
        left.append((cx + (R + w) * ux, cy + (R + w) * uy))
        right.append((cx + (R - w) * ux, cy + (R - w) * uy))
    m = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(m)
    d.polygon(left + right[::-1], fill=255)
    # наконечник: трикутник уздовж дотичної в кінці дуги
    a = math.radians(a1)
    sgn = 1 if a1 > a0 else -1
    tx, ty = -math.sin(a) * sgn, math.cos(a) * sgn   # напрямок руху
    ux, uy = math.cos(a), math.sin(a)                # назовні від центра
    bx, by = cx + R * ux, cy + R * uy
    hw, hl = head * size / 2, head * size * 0.95
    d.polygon([(bx + ux * hw, by + uy * hw), (bx - ux * hw, by - uy * hw), (bx + tx * hl, by + ty * hl)], fill=255)
    return m


def keycap(size, box, radius, c0, c1):
    """Скляна клавіша: градієнт, світла смуга зверху, тонка світла рамка. Повертає (RGB-шар, маска)."""
    x0, y0, x1, y1 = [int(v * size) for v in box]
    mask = Image.new("L", (size, size), 0)
    ImageDraw.Draw(mask).rounded_rectangle([x0, y0, x1, y1], int(radius * size), fill=255)
    face = diag_gradient(size, [(0, c0), (1, c1)], 55)
    # нижня частина трохи темніша — об'єм
    shade = diag_gradient(size, [(0, (255, 255, 255)), (1, (190, 190, 215))], 90)
    face = ImageChops.multiply(face, shade)
    # верхній відблиск
    hl = Image.new("L", (size, size), 0)
    ImageDraw.Draw(hl).rounded_rectangle(
        [x0 + int(size * 0.012), y0 + int(size * 0.012), x1 - int(size * 0.012), y0 + int((y1 - y0) * 0.46)],
        int(radius * size * 0.8), fill=70)
    hl = ImageChops.multiply(hl, mask)
    face.paste(Image.new("RGB", (size, size), (255, 255, 255)), (0, 0), hl)
    # рамка
    inner = mask.filter(ImageFilter.MinFilter(max(3, int(size * 0.008)) | 1))
    rim = ImageChops.subtract(mask, inner)
    face.paste(Image.new("RGB", (size, size), (235, 245, 255)), (0, 0), rim.point(lambda v: int(v * 0.75)))
    return face, mask


def neon_art(size=BIG, small=False):
    S = size
    # 1) темна плитка з синім сяйвом у нижньому правому куті й фіолетовим у лівому верхньому
    bg = diag_gradient(S, [(0, BG_A), (1, BG_B)], 55)
    bg = ImageChops.screen(bg, radial_glow(S, (0.95, 0.98), 0.75, (30, 100, 255), 0.55))
    bg = ImageChops.screen(bg, radial_glow(S, (0.08, 0.05), 0.55, (120, 40, 220), 0.35))
    img = bg

    if small:
        # спрощений варіант для 16–32 px: дві великі клавіші без стрілок
        k_a, m_a = keycap(S, (0.075, 0.25, 0.495, 0.75), 0.11, CYAN, BLUE)
        k_y, m_y = keycap(S, (0.505, 0.25, 0.925, 0.75), 0.11, VIOLET, MAGENTA)
        glow_layer = Image.new("RGB", (S, S), (0, 0, 0))
        glow_layer.paste(k_a, (0, 0), m_a)
        glow_layer.paste(k_y, (0, 0), m_y)
        img = screen_glow(img, glow_layer, [(S * 0.03, 0.9)])
        img.paste(k_a, (0, 0), m_a)
        img.paste(k_y, (0, 0), m_y)
        font = ImageFont.truetype(FONT, int(S * 0.43))
        d = ImageDraw.Draw(img)
        d.text((S * 0.285, S * 0.515), "A", font=font, fill=(255, 255, 255), anchor="mm")
        d.text((S * 0.715, S * 0.515), "Я", font=font, fill=(255, 255, 255), anchor="mm")
    else:
        C, R = (0.5, 0.47), 0.335
        # 2) стрілки-«перемикач»: дві дуги, кожна закінчується наконечником
        r1 = arc_ribbon(S, C, R, 196, 338, 0.014, 0.060, 0.135)
        r2 = arc_ribbon(S, C, R, 16, 158, 0.014, 0.060, 0.135)
        g1 = diag_gradient(S, [(0, (110, 60, 255)), (0.5, BLUE), (1, CYAN)], 0)
        g2 = diag_gradient(S, [(0, MAGENTA), (0.5, VIOLET), (1, (110, 80, 255))], 180)
        ribbons = Image.new("RGB", (S, S), (0, 0, 0))
        ribbons.paste(g1, (0, 0), r1)
        ribbons.paste(g2, (0, 0), r2)
        img = screen_glow(img, ribbons, [(S * 0.05, 1.0), (S * 0.016, 0.9)])
        img.paste(ribbons, (0, 0), ImageChops.lighter(r1, r2))

        # 3) клавіші «A» і «Я»
        k_a, m_a = keycap(S, (0.215, 0.335, 0.485, 0.605), 0.062, CYAN, BLUE)
        k_y, m_y = keycap(S, (0.515, 0.335, 0.785, 0.605), 0.062, VIOLET, MAGENTA)
        keys = Image.new("RGB", (S, S), (0, 0, 0))
        keys.paste(k_a, (0, 0), m_a)
        keys.paste(k_y, (0, 0), m_y)
        img = screen_glow(img, keys, [(S * 0.055, 1.0), (S * 0.018, 0.75)])
        img.paste(k_a, (0, 0), m_a)
        img.paste(k_y, (0, 0), m_y)
        font = ImageFont.truetype(FONT, int(S * 0.19))
        txt = Image.new("RGB", (S, S), (0, 0, 0))
        td = ImageDraw.Draw(txt)
        td.text((S * 0.35, S * 0.475), "A", font=font, fill=(255, 255, 255), anchor="mm")
        td.text((S * 0.65, S * 0.475), "Я", font=font, fill=(255, 255, 255), anchor="mm")
        img = screen_glow(img, txt, [(S * 0.012, 0.7)])
        d = ImageDraw.Draw(img)
        d.text((S * 0.35, S * 0.475), "A", font=font, fill=(255, 255, 255), anchor="mm")
        d.text((S * 0.65, S * 0.475), "Я", font=font, fill=(255, 255, 255), anchor="mm")

        # 4) нижній ряд «клавіатури»: чотири клавіші й довгий пробіл
        row = Image.new("RGB", (S, S), (0, 0, 0))
        ky, kh = 0.838, 0.056
        cells = ((0.205, 0.285, CYAN), (0.305, 0.385, BLUE), (0.405, 0.595, VIOLET), (0.615, 0.695, VIOLET), (0.715, 0.795, MAGENTA))
        for x0, x1, c in cells:
            cell = Image.new("RGB", (S, S), (0, 0, 0))
            m = Image.new("L", (S, S), 0)
            ImageDraw.Draw(m).rounded_rectangle([x0 * S, ky * S, x1 * S, (ky + kh) * S], 0.02 * S, fill=255)
            face = diag_gradient(S, [(0, c), (1, lerp(c, (255, 255, 255), 0.25))], 90)
            row.paste(face, (0, 0), m)
        img = screen_glow(img, row, [(S * 0.03, 1.0), (S * 0.01, 0.7)])
        img.paste(row, (0, 0), row.convert("L").point(lambda v: 255 if v > 8 else 0))

        # 4b) кілька «зірочок»
        sp = Image.new("RGB", (S, S), (0, 0, 0))
        sd = ImageDraw.Draw(sp)
        for (x, y, r, k) in ((0.86, 0.14, 0.008, 255), (0.12, 0.60, 0.006, 200), (0.90, 0.58, 0.005, 180), (0.17, 0.12, 0.005, 190)):
            sd.ellipse([(x - r) * S, (y - r) * S, (x + r) * S, (y + r) * S], fill=(k, k, 255))
        img = screen_glow(img, sp, [(S * 0.01, 1.0)])
        img = ImageChops.screen(img, sp)

    # 5) рамка плитки й маска заокруглення
    margin, radius = int(S * 0.045), int(S * 0.215)
    body = rounded_mask(S, margin, radius)
    rim = ImageChops.subtract(body, body.filter(ImageFilter.MinFilter(max(3, int(S * 0.012)) | 1)))
    rim_color = diag_gradient(S, [(0, (120, 140, 255)), (1, (80, 200, 255))], 45)
    img.paste(rim_color, (0, 0), rim.point(lambda v: int(v * 0.55)))
    out = img.convert("RGBA")
    out.putalpha(body)
    return out


def neon_paused(size=BIG, small=False):
    color = neon_art(size, small)
    a = color.getchannel("A")
    g = ImageOps.grayscale(color.convert("RGB")).point(lambda v: int(v * 0.8 + 18)).convert("RGBA")
    g.putalpha(a)
    return add_pause_badge(g, small)


# ───────────────────────────── стиль «lime» ─────────────────────────────

def squircle(size, box, n=4.6):
    """Маска «супереліпса» (як у значків iOS): box — (x0, y0, x1, y1) у частках розміру."""
    x0, y0, x1, y1 = box
    ys, xs = np.mgrid[0:size, 0:size].astype(np.float32)
    cx, cy, hw, hh = (x0 + x1) / 2 * size, (y0 + y1) / 2 * size, (x1 - x0) / 2 * size, (y1 - y0) / 2 * size
    f = (np.abs(xs + 0.5 - cx) / hw) ** n + (np.abs(ys + 0.5 - cy) / hh) ** n
    return Image.fromarray(((f <= 1.0) * 255).astype(np.uint8), "L")


def shadow_of(mask, dy, blur, alpha):
    """Розмита тінь від маски: чорний шар RGBA, зсунутий униз."""
    size = mask.size[0]
    sh = Image.new("L", (size, size), 0)
    sh.paste(mask.point(lambda v: int(v * alpha / 255)), (0, int(dy * size)))
    sh = sh.filter(ImageFilter.GaussianBlur(blur * size))
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.putalpha(sh)
    return out


def draw_yi(d, cx, cy, h, color):
    """Велика «Ї» із засічками: дві крапки над «І». (cx, cy) — центр знака, h — його висота."""
    u = h / 100
    top = cy - h / 2
    for dx in (-17 * u, 17 * u):  # крапки
        d.ellipse([cx + dx - 9.5 * u, top, cx + dx + 9.5 * u, top + 19 * u], fill=color)
    sw, sh, st = 16 * u, 66 * u, 11 * u  # стовбур, висота «І», товщина засічки
    y0 = top + 100 * u - sh
    d.rectangle([cx - sw / 2, y0 + st / 2, cx + sw / 2, top + 100 * u - st / 2], fill=color)
    for y in (y0, top + 100 * u - st):  # верхня й нижня засічки
        d.rounded_rectangle([cx - 30 * u, y, cx + 30 * u, y + st], 2.5 * u, fill=color)


def lime_art(size=BIG, small=False):
    S = size
    margin, radius = int(S * 0.045), int(S * 0.215)
    tile_mask = rounded_mask(S, margin, radius)

    # темна плитка: угорі світліша, внизу майже чорна
    tile_rgb = diag_gradient(S, [(0, (66, 67, 70)), (1, (22, 23, 25))], 90).convert("RGBA")
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste(tile_rgb, (0, 0), tile_mask)

    # геометрія: у малому варіанті менше клавіш і більший «підказковий» бабл
    if small:
        body_box = (0.09, 0.42, 0.91, 0.91)
        kh, gap = 0.135, 0.05
        row1 = [(0.20 + i * 0.24, 0.20 + i * 0.24 + 0.20) for i in range(3)]
        y1 = 0.485
        row2 = [(0.18, 0.37), (0.43, 0.82 - 0.19 - 0.06 + 0.19 - 0.19), (0.63, 0.82)]
        row2 = [(0.17, 0.35), (0.41, 0.59), (0.65, 0.83)]
        y2 = 0.485 + kh + gap
        rows = [(y1, row1), (y2, row2)]
        bubble = (0.27, 0.035, 0.73, 0.47)
        tip = (0.5, 0.56, 0.10)
        glyph_h, glyph_y = 0.33, 0.25
    else:
        body_box = (0.12, 0.385, 0.88, 0.875)
        kh, gap = 0.092, 0.040
        kw, kg = 0.095, 0.03125
        x_start = 0.20
        row1 = [(x_start + i * (kw + kg), x_start + i * (kw + kg) + kw) for i in range(5)]
        x2 = 0.5 - (4 * kw + 3 * kg) / 2
        row2 = [(x2 + i * (kw + kg), x2 + i * (kw + kg) + kw) for i in range(4)]
        row3 = [(0.20, 0.20 + kw), (0.20 + kw + kg, 0.80 - kw - kg), (0.80 - kw, 0.80)]
        rows = [(0.435, row1), (0.435 + kh + gap, row2), (0.435 + 2 * (kh + gap), row3)]
        bubble = (0.345, 0.075, 0.655, 0.405)
        tip = (0.5, 0.462, 0.088)
        glyph_h, glyph_y = 0.235, 0.24

    # клавіатура: тінь, тіло з градієнтом, світла верхня кромка
    body = Image.new("L", (S, S), 0)
    ImageDraw.Draw(body).rounded_rectangle([int(v * S) for v in body_box], int(S * (0.135 if not small else 0.15)), fill=255)
    img = Image.alpha_composite(img, shadow_of(body, 0.02, 0.022, 150))
    green = diag_gradient(S, [(0, (232, 255, 70)), (0.5, (112, 248, 58)), (1, (16, 214, 50))], 40).convert("RGBA")
    img.paste(green, (0, 0), body)
    top_light = Image.new("RGBA", (S, S), (255, 255, 255, 0))
    top_light.putalpha(diag_gradient(S, [(0, (110, 110, 110)), (0.5, (0, 0, 0)), (1, (0, 0, 0))], 90).convert("L").filter(ImageFilter.GaussianBlur(S * 0.01)))
    top_light.putalpha(ImageChops.multiply(top_light.getchannel("A"), body))
    img = Image.alpha_composite(img, top_light)

    # клавіші-«вікна»: темні заглибини (угорі темніше), знизу тонкий світлий край
    r = int(S * (0.028 if small else 0.024))
    lip = max(2, int(S * 0.006))
    for y, row in rows:
        for x0, x1 in row:
            box = [int(x0 * S), int(y * S), int(x1 * S), int((y + kh) * S)]
            lip_mask = Image.new("L", (S, S), 0)
            ImageDraw.Draw(lip_mask).rounded_rectangle([box[0], box[1] + lip, box[2], box[3] + lip], r, fill=255)
            img.paste(Image.new("RGBA", (S, S), (200, 255, 130, 120)), (0, 0), lip_mask)
            key_mask = Image.new("L", (S, S), 0)
            ImageDraw.Draw(key_mask).rounded_rectangle(box, r, fill=255)
            w, h = box[2] - box[0], box[3] - box[1]
            strip = Image.new("RGB", (1, h))
            for yy in range(h):
                strip.putpixel((0, yy), tuple(round(a + (b - a) * yy / max(1, h - 1)) for a, b in zip((9, 11, 9), (34, 38, 34))))
            face = Image.new("RGBA", (S, S), (0, 0, 0, 0))
            face.paste(strip.resize((w, h)).convert("RGBA"), (box[0], box[1]))
            img.paste(face, (0, 0), key_mask)

    # біла «підказка» над клавішею (як при натисканні на телефоні): заокруглений прямокутник + «кінчик» донизу
    bx0, by0, bx1, by1 = [int(v * S) for v in bubble]
    bm = Image.new("L", (S, S), 0)
    bd = ImageDraw.Draw(bm)
    bd.rounded_rectangle([bx0, by0, bx1, by1], int(S * 0.085), fill=255)
    tcx, tcy, th = int(tip[0] * S), int(tip[1] * S), int(tip[2] * S)
    bd.polygon([(tcx - th * 1.25, tcy - th * 1.6), (tcx + th * 1.25, tcy - th * 1.6), (tcx + th * 0.22, tcy + th * 0.0), (tcx - th * 0.22, tcy + th * 0.0)], fill=255)
    # злегка скруглюємо стики й вістря
    bm = bm.filter(ImageFilter.GaussianBlur(S * 0.011)).point(lambda v: 255 if v > 132 else (0 if v < 124 else int((v - 124) * 255 / 8)))
    img = Image.alpha_composite(img, shadow_of(bm, 0.014, 0.014, 150))
    paper = diag_gradient(S, [(0, (255, 255, 255)), (1, (226, 230, 226))], 90).convert("RGBA")
    img.paste(paper, (0, 0), bm)
    draw_yi(ImageDraw.Draw(img), S * 0.5, S * glyph_y, S * glyph_h, (18, 21, 18, 255))

    # усе, що виступає за плитку, обрізаємо
    clip = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    clip.paste(img, (0, 0), tile_mask)
    return clip


def lime_paused(size=BIG, small=False):
    color = lime_art(size, small)
    a = color.getchannel("A")
    g = ImageOps.grayscale(color.convert("RGB")).point(lambda v: int(v * 0.8 + 18)).convert("RGBA")
    g.putalpha(a)
    return add_pause_badge(g, small)


# ───────────────────────────── стиль «dual» ─────────────────────────────

SOURCE_DUAL = OUT / "icon_dual_source.png"


def dual_source_art(size=BIG):
    """Готовий малюнок: обрізаємо прозоре поле, вписуємо плитку в квадрат із тим самим відступом, що й в інших стилів."""
    src = Image.open(SOURCE_DUAL).convert("RGBA")
    box = src.getchannel("A").point(lambda v: 255 if v > 128 else 0).getbbox()
    tile = src.crop(box)
    side = size - 2 * int(size * 0.045)
    k = side / max(tile.size)
    tile = tile.resize((max(1, round(tile.size[0] * k)), max(1, round(tile.size[1] * k))), Image.LANCZOS)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(tile, ((size - tile.size[0]) // 2, (size - tile.size[1]) // 2))
    return out


def _board(img, box, radius, c0, c1, keys, key_radius, shadow=True):
    """Одна клавіатура: тінь, тіло з градієнтом, темні клавіші-«вікна». box і keys — у частках полотна (keys — у частках box)."""
    S = img.size[0]
    x0, y0, x1, y1 = [v * S for v in box]
    body = Image.new("L", (S, S), 0)
    ImageDraw.Draw(body).rounded_rectangle([x0, y0, x1, y1], int(radius * S), fill=255)
    if shadow:
        img.alpha_composite(shadow_of(body, 0.018, 0.02, 150))
    img.paste(diag_gradient(S, [(0, c0), (1, c1)], 50).convert("RGBA"), (0, 0), body)
    d = ImageDraw.Draw(img)
    w, h = x1 - x0, y1 - y0
    for kx0, ky0, kx1, ky1 in keys:
        d.rounded_rectangle([x0 + kx0 * w, y0 + ky0 * h, x0 + kx1 * w, y0 + ky1 * h], int(key_radius * S), fill=(24, 26, 28, 255))


def dual_art(size=BIG, small=False):
    if not small:
        return dual_source_art(size)
    S = size
    tile_mask = rounded_mask(S, int(S * 0.045), int(S * 0.215))
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste(diag_gradient(S, [(0, (62, 63, 70)), (1, (22, 23, 28))], 90).convert("RGBA"), (0, 0), tile_mask)

    def rel(r):  # частка плитки -> частка полотна
        return 0.045 + 0.91 * r

    def three(y0, y1, p=0.12, g=0.075):
        w = (1 - 2 * p - 2 * g) / 3
        return [(p + i * (w + g), y0, p + i * (w + g) + w, y1) for i in range(3)]

    # позаду: сіро-синя, видно верхній ряд і лівий край
    back = (rel(0.08), rel(0.13), rel(0.76), rel(0.66))
    _board(img, back, 0.12, (136, 172, 214), (56, 76, 110), three(0.12, 0.34, 0.10, 0.07) + [(0.10, 0.48, 0.30, 0.70)], 0.022)
    # попереду: салатова, два ряди (три клавіші й пробіл)
    front = (rel(0.24), rel(0.36), rel(0.91), rel(0.87))
    _board(img, front, 0.12, (228, 255, 64), (20, 222, 40), three(0.17, 0.41, 0.17, 0.14) + [(0.17, 0.57, 0.83, 0.81)], 0.026)
    clip = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    clip.paste(img, (0, 0), tile_mask)
    return clip


def dual_paused(size=BIG, small=False):
    color = dual_art(size, small)
    a = color.getchannel("A")
    g = ImageOps.grayscale(color.convert("RGB")).point(lambda v: int(v * 0.8 + 18)).convert("RGBA")
    g.putalpha(a)
    return add_pause_badge(g, small)


def add_pause_badge(img, small):
    S = img.size[0]
    d = ImageDraw.Draw(img)
    r = int(S * (0.27 if small else 0.2))
    cx = cy = int(S * (0.70 if small else 0.75))
    d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=(30, 34, 46, 255), outline=(255, 255, 255, 255), width=int(S * 0.018))
    bw, bh, gap = int(S * (0.07 if small else 0.05)), int(S * (0.26 if small else 0.19)), int(S * (0.06 if small else 0.042))
    for dx in (-(gap // 2 + bw), gap // 2):
        d.rounded_rectangle([cx + dx, cy - bh // 2, cx + dx + bw, cy + bh // 2], int(S * 0.012), fill=(255, 255, 255, 255))
    return img



def shrink(img, px):
    return img.resize((px, px), Image.LANCZOS)


# ───────────────────────────── запис .ico ─────────────────────────────

def dib_entry(img):
    """32-бітний BMP-запис для .ico (так значки малих розмірів відкриваються скрізь)."""
    w, h = img.size
    px = img.convert("RGBA").load()
    pixels = bytearray()
    for y in range(h - 1, -1, -1):  # рядки знизу догори
        for x in range(w):
            r, g, b, a = px[x, y]
            pixels += bytes((b, g, r, a))
    mask_row = ((w + 31) // 32) * 4
    mask = bytes(mask_row * h)  # прозорість несе альфа-канал, маска нульова
    header = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, len(pixels) + len(mask), 0, 0, 0, 0)
    return header + bytes(pixels) + mask


def png_entry(img):
    buf = BytesIO()
    img.save(buf, "PNG", optimize=True)
    return buf.getvalue()


def write_ico(path, art, art_small):
    entries = []
    for s in SIZES:
        im = shrink(art_small if s <= 32 else art, s)
        data = png_entry(im) if s >= 64 else dib_entry(im)
        entries.append((s, data))
    out = bytearray(struct.pack("<HHH", 0, 1, len(entries)))
    offset = 6 + 16 * len(entries)
    for s, data in entries:
        out += struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for _, data in entries:
        out += data
    path.write_bytes(bytes(out))


def preview(arts, path):
    """Лист для перегляду: великі значки, а нижче справжні розміри (і збільшені в 4 рази) на світлому й темному тлі."""
    on, off, on_s, off_s = arts
    W, H = 1180, 620
    sheet = Image.new("RGB", (W, H), (245, 246, 248))
    ImageDraw.Draw(sheet).rectangle([W // 2, 300, W, H], fill=(32, 34, 38))
    for i, art in enumerate((on, off)):
        big = shrink(art, 256)
        sheet.paste(big, (60 + i * 300, 24), big)
    sizes = (16, 20, 24, 32)
    for panel, bg in enumerate(((245, 246, 248), (32, 34, 38))):
        x0 = 30 + panel * (W // 2)
        for row, (art, art_s) in enumerate(((on, on_s), (off, off_s))):
            y0 = 330 + row * 140
            x = x0
            for s in sizes:
                im = shrink(art_s if s <= 32 else art, s)
                sheet.paste(im, (x, y0 + 4), im)  # справжній розмір
                zoom = Image.new("RGB", (s, s), bg)
                zoom.paste(im, (0, 0), im)
                zoom = zoom.resize((s * 4, s * 4), Image.NEAREST)
                sheet.paste(zoom, (x + s + 8, y0))
                x += s * 5 + 28
    sheet.save(path)


def main():
    style = sys.argv[1] if len(sys.argv) > 1 else "dual"
    if style not in ("dual", "lime", "neon", "flag"):
        sys.exit("стиль: dual, lime, neon або flag")
    out_dir = Path(sys.argv[2]) if len(sys.argv) > 2 else OUT
    if not Path(FONT).exists():
        sys.exit(f"нема шрифту {FONT}")
    out_dir.mkdir(parents=True, exist_ok=True)
    if style == "neon":
        on, off = neon_art(), neon_paused()
        on_s, off_s = neon_art(small=True), neon_paused(small=True)
    elif style == "dual":
        on, off = dual_art(), dual_paused()
        on_s, off_s = dual_art(small=True), dual_paused(small=True)
    elif style == "lime":
        on, off = lime_art(), lime_paused()
        on_s, off_s = lime_art(small=True), lime_paused(small=True)
    else:
        on, off = base_art(), paused_art()
        on_s, off_s = base_art(small=True), paused_art(small=True)
    write_ico(out_dir / "icon.ico", on, on_s)
    write_ico(out_dir / "icon_off.ico", off, off_s)
    preview((on, off, on_s, off_s), out_dir / "preview.png")
    print(f"готово ({style}, {out_dir}):", ", ".join(p.name for p in sorted(out_dir.glob("icon*.ico"))), "+ preview.png")


if __name__ == "__main__":
    main()
