#!/usr/bin/env python3
"""Generates Dusty's UI icon set (48x48, chunky flat style).

System:
  * Every solid part is a "chunk": an edge-colored underlay (outline, 3px each side)
    plus the same underlay shifted 3px down (the chunky bottom lip), then the flat fill.
    Drawing the outline as an underlay merges unions cleanly (gear teeth, wrench head).
  * Chunky lines (arrows, ticks): edge stroke w+6 under a fill stroke w.
  * Inner details: 3px strokes in edge color, or soft / white marks.
  * Highlight: white pill at ~55% opacity, top-left (light from top-left).
"""
import math, os, sys

OUT = sys.argv[1]
os.makedirs(OUT, exist_ok=True)

T = {
    "grape": ("#7C5CFF", "#5B3FD9", "#EFEAFF"),
    "coral": ("#FF5A6E", "#D63D52", "#FFE5E9"),
    "sun":   ("#FFC83D", "#E0A100", "#FFF4D6"),
    "sky":   ("#3DB8FF", "#1E93D6", "#E3F5FF"),
    "teal":  ("#2EC4B6", "#1E9C90", "#DDF7F4"),
    "blue":  ("#5B8DEF", "#3F6FD1", "#E6EEFF"),
    "mint":  ("#22C993", "#169E72", "#DFF8EE"),
}
W = "#FFFFFF"
OUTLINE = 3      # px each side
SHADE = 3        # px bottom lip
LINE = 3         # inner detail stroke


def B(t): return T[t][0]
def E(t): return T[t][1]
def S(t): return T[t][2]


class Sh:
    """A solid shape: tag + geometry attrs. pad rounds corners by growing with a same-color stroke."""
    def __init__(self, tag, color=None, pad=0, **a):
        self.tag, self.color, self.pad, self.a = tag, color, pad, a

    def svg(self, fill, stroke=None, sw=0):
        attrs = " ".join(f'{k.replace("_", "-")}="{v}"' for k, v in self.a.items())
        s = f'<{self.tag} {attrs} fill="{fill}"'
        if stroke and sw:
            s += f' stroke="{stroke}" stroke-width="{sw:g}"'
        return s + "/>"


class Ln:
    """A chunky line (path d)."""
    def __init__(self, d, w, color=None):
        self.d, self.w, self.color = d, w, color


def rect(x, y, w, h, r, color=None, pad=0):
    return Sh("rect", color, pad, x=x, y=y, width=w, height=h, rx=r)
def circ(cx, cy, r, color=None): return Sh("circle", color, 0, cx=cx, cy=cy, r=r)
def ell(cx, cy, rx, ry, color=None, **kw): return Sh("ellipse", color, 0, cx=cx, cy=cy, rx=rx, ry=ry, **kw)
def path(d, color=None, pad=0): return Sh("path", color, pad, d=d)


def chunk(tone, shapes, transform=None, shade=True, fill=None):
    """Outline + shade underlay, then fills. `fill` overrides default fill (base)."""
    e = E(tone)
    base = fill or B(tone)
    tf = f' transform="{transform}"' if transform else ""

    def under():
        out = []
        for s in shapes:
            if isinstance(s, Ln):
                out.append(f'<path d="{s.d}" fill="none" stroke="{e}" stroke-width="{s.w + 2*OUTLINE:g}"/>')
            else:
                out.append(s.svg(e, e, 2*OUTLINE + 2*s.pad))
        return "".join(out)

    parts = []
    if shade:
        st = f"translate(0 {SHADE})" + (f" {transform}" if transform else "")
        parts.append(f'<g transform="{st}">{under()}</g>')
    parts.append(f"<g{tf}>{under()}")
    for s in shapes:
        c = s.color or base
        if isinstance(s, Ln):
            parts.append(f'<path d="{s.d}" fill="none" stroke="{c}" stroke-width="{s.w:g}"/>')
        else:
            parts.append(s.svg(c, c if s.pad else None, 2*s.pad))
    parts.append("</g>")
    return "".join(parts)


def line(d, color, w=LINE, transform=None):
    tf = f' transform="{transform}"' if transform else ""
    return f'<path d="{d}" fill="none" stroke="{color}" stroke-width="{w:g}"{tf}/>'


def hl(x, y, w, h, transform=None, op=0.6):
    tf = f' transform="{transform}"' if transform else ""
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{min(w, h)/2:g}" fill="{W}" opacity="{op}"{tf}/>'


def dot(cx, cy, r=1.6, op=0.6):
    return f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="{W}" opacity="{op}"/>'


def outline_only(shape, color, w=LINE):
    return shape.svg("none", color, w)


def P(x, y): return f"{x:.2f} {y:.2f}"


ICONS = {}
def icon(name):
    def deco(f):
        ICONS[name] = f
        return f
    return deco

# ---------------------------------------------------------------- groups

@icon("build")
def _():
    t = "coral"
    dome = path("M9 31 C9 20 15.5 12.5 24 12.5 C32.5 12.5 39 20 39 31 Z")
    brim = rect(4.5, 30, 39, 8, 4)
    return (chunk(t, [dome, brim])
            + rect(20, 9.5, 8, 21, 4, S(t)).svg(S(t), E(t), LINE)
            + line("M13 30 V27", E(t)) + line("M35 30 V27", E(t))
            + hl(13, 19, 4, 7, "rotate(30 15 22.5)"))


@icon("cache-dev")
def _():
    t = "sun"
    body = rect(8, 5, 32, 35, 6)
    out = chunk(t, [body])
    for y, h in ((9.5, 12), (25, 11)):
        out += rect(12.5, y, 23, h, 3).svg(S(t), E(t), LINE)
        out += line(f"M20 {y + h/2:g} H28", E(t), 3.5)
    out += rect(12, 40, 6, 4, 2, E(t)).svg(E(t)) + rect(30, 40, 6, 4, 2).svg(E(t))
    return out


@icon("cache-app")
def _():
    t = "sky"
    win = rect(4.5, 7, 39, 33, 6, S(t))
    out = chunk(t, [win])
    out += path("M4.5 17 V13 A6 6 0 0 1 10.5 7 H37.5 A6 6 0 0 1 43.5 13 V17 Z").svg(B(t))
    out += line("M4.5 17 H43.5", E(t))
    out += "".join(f'<circle cx="{x}" cy="12" r="2" fill="{W}"/>' for x in (10.5, 16.5, 22.5))
    # globe in the page
    out += circ(24, 28.5, 7.5).svg(B(t), E(t), LINE)
    out += line("M16.5 28.5 H31.5", E(t), 2.5)
    out += ell(24, 28.5, 3, 7.5).svg("none", E(t), 2.5)
    return out


@icon("junk")
def _():
    t = "teal"
    zig = "".join(f" L{37 - 26/6*(i+1):.2f} {41 if i % 2 == 0 else 37.5}" for i in range(6))
    receipt = path(f"M11 6.5 H37 V37.5{zig} Z", pad=1)
    out = chunk(t, [receipt])
    for y, x2 in ((14, 31), (20.5, 26), (27, 31)):
        out += line(f"M17 {y} H{x2}", S(t), 3.5)
    out += hl(14, 9, 7, 3, op=0.45)
    return out


@icon("docker")
def _():
    t = "blue"
    box = rect(5, 19, 38, 21, 6)
    out = chunk(t, [box])
    for x in (21, 28.5, 36):
        out += line(f"M{x} 24 V35", S(t), 3.5)
    out += circ(12.5, 27, 3.6).svg(W, E(t), 2.5) + circ(13.2, 27.6, 1.6).svg(E(t))
    # spout
    tail = [Ln("M35 19 V13", 4.5),
            path("M28 5.5 C30.5 9 33 10.5 35 11 C37 10.5 39.5 9 42 5.5 C42 10.5 39 14.5 35 14.5 C31 14.5 28 10.5 28 5.5 Z", pad=1)]
    out = chunk(t, tail) + out
    out += chunk("sky", [path("M14 7 C15.5 9.5 16.5 11 16.5 12.5 A2.5 2.5 0 0 1 11.5 12.5 C11.5 11 12.5 9.5 14 7 Z")], shade=False)
    out += chunk("sky", [path("M21 11 C22.2 13 23 14.2 23 15.3 A2 2 0 0 1 19 15.3 C19 14.2 19.8 13 21 11 Z")], shade=False)
    return out


@icon("redownload")
def _():
    t = "grape"
    box = rect(7.5, 16, 33, 26, 5)
    lid = rect(5, 10, 38, 10, 4)
    out = chunk(t, [box]) + chunk(t, [lid])
    arrow = path("M20.5 22.5 H27.5 V29 H32.5 L24 38 L15.5 29 H20.5 Z", S(t), pad=1)
    out += arrow.svg(S(t), E(t), LINE)
    out += hl(9, 12.5, 9, 3, op=0.5)
    return out

# ---------------------------------------------------------------- actions

@icon("reveal")
def _():
    t = "sky"
    back = path("M5 13 A4 4 0 0 1 9 9 H17 A3 3 0 0 1 19.4 10.2 L21.5 13 H37 A4 4 0 0 1 41 17 V36 H5 Z", E(t))
    front = rect(5, 16.5, 38, 23, 5)
    out = chunk(t, [back, front])
    out += hl(9, 20, 9, 3, op=0.5)
    # magnifier
    out += chunk("grape", [Ln("M36.5 36.5 L41.5 41.5", 4.5)], shade=False)
    out += chunk("grape", [circ(31, 31, 7, S("grape"))], shade=False)
    out += dot(28.6, 28.6, 1.7, 0.9)
    return out


@icon("never")
def _():
    t = "coral"
    shield = path("M24 4.5 L39.5 10 V22 C39.5 32 33 38.5 24 43 C15 38.5 8.5 32 8.5 22 V10 Z", pad=1)
    out = chunk(t, [shield])
    # paw in soft
    c = S(t)
    out += ell(24, 28.5, 5.6, 4.6).svg(c)
    for cx, cy in ((16.8, 22.3), (21.2, 17.3), (26.8, 17.3), (31.2, 22.3)):
        out += ell(cx, cy, 2.4, 2.9).svg(c)
    return out


def bin_shapes(top=17, bottom=42):
    body = path(f"M11 {top} H37 L35 {bottom-4} A4 4 0 0 1 31 {bottom} H17 A4 4 0 0 1 13 {bottom-4} Z")
    return body


@icon("trash")
def _():
    t = "coral"
    out = chunk(t, [bin_shapes(16, 42)])
    out += chunk(t, [Ln("M20 9 V7.5 A2 2 0 0 1 22 5.5 H26 A2 2 0 0 1 28 7.5 V9", 3, E(t)), rect(7, 9, 34, 6.5, 3.25)])
    for x in (19.5, 24, 28.5):
        out += line(f"M{x} 22 V35", S(t), 3.5)
    out += hl(10, 10.5, 8, 2.5, op=0.55)
    return out


@icon("put-back")
def _():
    t = "grape"
    out = chunk(t, [path("M11 27 H37 L35 39 A4 4 0 0 1 31 43 H17 A4 4 0 0 1 13 39 Z")])
    out += chunk("mint", [Ln("M29 26 V16.5 A6.5 6.5 0 0 0 22.5 10 H13 M18 5 L13 10 L18 15", 5)])
    out += chunk(t, [rect(7, 23, 34, 6.5, 3.25)])
    for x in (20, 28):
        out += line(f"M{x} 33.5 V37.5", S(t), 3.5)
    return out


@icon("search")
def _():
    t = "grape"
    out = chunk(t, [Ln("M30.5 30.5 L39.5 39.5", 7)])
    out += chunk(t, [circ(21, 21, 14)])
    out += circ(21, 21, 8.5).svg(S(t), E(t), LINE)
    out += hl(15.5, 16, 3, 6.5, "rotate(40 17 19.25)", 0.95)
    return out


@icon("settings")
def _():
    t = "blue"
    cx, cy = 24, 23.5
    teeth = [rect(20, 4.5, 8, 10, 2.5) for _ in range(8)]
    out = []
    shapes = [circ(cx, cy, 13.5)]
    for i, r in enumerate(teeth):
        r.a["transform"] = f"rotate({i*45} {cx} {cy})"
        shapes.append(r)
    s = chunk(t, shapes)
    s += circ(cx, cy, 5.5).svg(S(t), E(t), LINE)
    s += hl(14, 15, 3, 7, f"rotate(35 15.5 18.5)", 0.55)
    return s


@icon("back")
def _():
    return chunk("grape", [Ln("M29 9.5 L15.5 23.5 L29 37.5", 6)])


@icon("check")
def _():
    return chunk("mint", [Ln("M10 25 L19.5 34 L38 14", 6.5)])


@icon("warning")
def _():
    t = "sun"
    tri = path("M24 7.5 L41.5 38.5 H6.5 Z", pad=3)
    out = chunk(t, [tri])
    out += chunk(t, [Ln("M24 18 V27", 4.5, W)], shade=False)
    out += f'<circle cx="24" cy="34" r="5.25" fill="{E(t)}"/><circle cx="24" cy="34" r="2.6" fill="{W}"/>'
    return out


@icon("lock")
def _():
    t = "grape"
    out = chunk(t, [Ln("M16 22 V15.5 A8 8 0 0 1 32 15.5 V22", 4.5, S(t))], shade=False)
    out += chunk(t, [rect(8.5, 20, 31, 22, 6)])
    out += f'<circle cx="24" cy="29" r="3.6" fill="{E(t)}"/>' + line("M24 30 V35.5", E(t), 4)
    out += hl(12, 23.5, 8, 3, op=0.55)
    return out


@icon("practice")
def _():
    t = "teal"
    flask = path("M20 9 V18.5 L10 35.5 A4.5 4.5 0 0 0 13.9 42.3 H34.1 A4.5 4.5 0 0 0 38 35.5 L28 18.5 V9 Z", S(t))
    out = chunk(t, [flask])
    out += path("M14.6 28 H33.4 L38 35.5 A4.5 4.5 0 0 1 34.1 42.3 H13.9 A4.5 4.5 0 0 1 10 35.5 Z").svg(B(t))
    out += line("M14.6 28 H33.4", E(t), 2.5)
    out += chunk(t, [rect(16, 5, 16, 6, 3)], shade=False)
    out += dot(20, 35, 2, 0.8) + dot(27, 37.5, 1.4, 0.8) + dot(23.5, 22.5, 1.5, 0)  # bubbles
    out += f'<circle cx="25" cy="22" r="1.8" fill="{B(t)}"/>'
    return out


def star4(cx, cy, r, k=0.28):
    # 4-point sparkle built from cubic curves
    i = r * k
    return (f"M{cx} {cy-r} C{cx+i*0.6} {cy-i} {cx+i} {cy-i*0.6} {cx+r} {cy} "
            f"C{cx+i} {cy+i*0.6} {cx+i*0.6} {cy+i} {cx} {cy+r} "
            f"C{cx-i*0.6} {cy+i} {cx-i} {cy+i*0.6} {cx-r} {cy} "
            f"C{cx-i} {cy-i*0.6} {cx-i*0.6} {cy-i} {cx} {cy-r} Z")


@icon("sparkle")
def _():
    t = "sun"
    out = chunk(t, [path(star4(21, 25, 16, 0.32), pad=1)])
    out += chunk(t, [path(star4(37.5, 10.5, 6, 0.32), pad=0.5)])
    out += hl(16, 18, 3, 6, "rotate(30 17.5 21)", 0.6)
    return out


@icon("paw")
def _():
    t = "grape"
    out = chunk(t, [path("M24 22.5 C30 22.5 36.5 30 36.5 35.5 C36.5 39.5 33.5 41.5 30 41 C27.5 40.6 26 39.5 24 39.5 C22 39.5 20.5 40.6 18 41 C14.5 41.5 11.5 39.5 11.5 35.5 C11.5 30 18 22.5 24 22.5 Z")])
    for cx, cy, rot in ((8.5, 21.5, -25), (17.5, 10.5, -10), (30.5, 10.5, 10), (39.5, 21.5, 25)):
        out += chunk(t, [ell(cx, cy, 4, 5, transform=f"rotate({rot} {cx} {cy})")])
    out += hl(16, 28, 3, 5.5, "rotate(35 17.5 30.75)", 0.55)
    return out


@icon("gift")
def _():
    t, r = "coral", "sun"
    out = chunk(r, [path("M24 13 C20 5 12 4.5 12.5 10 C13 13.5 19 14 24 13 Z"),
                    path("M24 13 C28 5 36 4.5 35.5 10 C35 13.5 29 14 24 13 Z")], shade=False)
    out += chunk(t, [rect(8.5, 20, 31, 22, 4)])
    out += chunk(t, [rect(6, 13, 36, 9, 3.5)])
    out += rect(20.5, 13, 7, 9, 0).svg(B(r)) + rect(20.5, 25, 7, 17, 0).svg(B(r))
    out += hl(9.5, 15.5, 7, 2.6, op=0.55)
    return out


@icon("party")
def _():
    t = "grape"
    cone = path("M7 41 L16.5 18 L30 31.5 Z", pad=2.5)
    out = chunk(t, [cone])
    out += line("M12.6 27.5 L21.2 36", S(t), 3.2) + line("M9.6 34.5 L14.2 39", S(t), 3.2)
    out += chunk("sun", [circ(37, 9, 3)], shade=False)
    out += chunk("coral", [rect(22, 6, 5.5, 5.5, 1.5)], shade=False).replace("<g>", '<g transform="rotate(20 24.75 8.75)">', 2) if False else ""
    out += chunk("coral", [rect(22, 6, 5.5, 5.5, 1.5)], transform="rotate(20 24.75 8.75)", shade=False)
    out += chunk("mint", [Ln("M30 21 C32 17 35 20 37 16", 3)], shade=False)
    out += chunk("sky", [circ(39.5, 29, 2.4)], shade=False)
    out += chunk("coral", [Ln("M23 15.5 L25.5 18", 3)], shade=False) if False else ""
    return out


@icon("broom")
def _():
    tf = "rotate(32 24 24)"
    out = chunk("coral", [Ln("M24 3 V22", 4.5)], transform=tf)
    out += chunk("sun", [path("M17.5 26 H30.5 L35.5 40 A2.5 2.5 0 0 1 33.1 43.3 H14.9 A2.5 2.5 0 0 1 12.5 40 Z")], transform=tf)
    out += line("M21.3 31 L20 39.5 M26.7 31 L28 39.5", E("sun"), 2.8, tf)
    out += chunk("grape", [rect(16.5, 20.5, 15, 6.5, 2.5)], transform=tf, shade=False)
    out += chunk("grape", [circ(8, 35, 2)], shade=False).replace(E("grape"), E("grape")) if False else ""
    out += f'<circle cx="7.5" cy="37" r="2.2" fill="{S("grape")}" stroke="{E("grape")}" stroke-width="2"/>'
    out += f'<circle cx="11.5" cy="42" r="1.5" fill="{S("grape")}" stroke="{E("grape")}" stroke-width="2"/>'
    return out


@icon("schedule")
def _():
    t = "grape"
    cal = rect(4.5, 8, 31, 29, 5, S(t))
    out = chunk(t, [cal])
    out += path("M4.5 17 V13 A5 5 0 0 1 9.5 8 H30.5 A5 5 0 0 1 35.5 13 V17 Z").svg(B(t))
    out += line("M4.5 17 H35.5", E(t))
    out += chunk(t, [Ln("M12.5 5 V10", 3.5, S(t)), Ln("M27.5 5 V10", 3.5, S(t))], shade=False)
    for x, y in ((11, 23), (17.5, 23), (11, 30)):
        out += rect(x - 1.75, y - 1.75, 3.5, 3.5, 1).svg(B(t))
    out += chunk("sun", [circ(33.5, 32.5, 10, S("sun"))])
    out += line("M33.5 27 V32.5 L37.5 35", E("sun"), 3.2)
    return out

# ---------------------------------------------------------------- tools

@icon("uninstall")
def _():
    b = "coral"
    out = chunk(b, [path("M10 30 H38 L36 39 A4 4 0 0 1 32 42.5 H16 A4 4 0 0 1 12 39 Z")])
    tf = "rotate(-14 23 15)"
    out += chunk("grape", [rect(13.5, 5.5, 19, 19, 5.5)], transform=tf)
    out += "".join(rect(x, y, 4.6, 4.6, 1.6).svg(S("grape")).replace("/>", f' transform="{tf}"/>') for x in (17.4, 24) for y in (9.4, 16))
    out += chunk(b, [rect(6, 26.5, 36, 6.5, 3.25)])
    out += line("M37 8 V13 M41.5 11 L38.5 14.5", S(b) if False else E("grape"), 2.6) if False else ""
    out += chunk("grape", [Ln("M40 12 V6 M37.5 8.5 L40 6 L42.5 8.5", 2.5, S("grape"))], shade=False)
    return out


@icon("big-files")
def _():
    t = "blue"
    doc = path("M8 8.5 A4 4 0 0 1 12 4.5 H26 L36 14.5 V38 A4 4 0 0 1 32 42 H12 A4 4 0 0 1 8 38 Z")
    out = chunk(t, [doc])
    out += path("M26 4.5 V11.5 A3 3 0 0 0 29 14.5 H36 Z").svg(S(t), E(t), 2.5)
    out += line("M13.5 18 H23 M13.5 24.5 H20", S(t), 3.5)
    # weight
    w = "sun"
    w = "coral"
    out += chunk(w, [Ln("M28 36 H42", 3.5, S(w))], shade=False)
    out += chunk(w, [rect(24, 29, 5.5, 14, 2.75)]) + chunk(w, [rect(39, 29, 5.5, 14, 2.75)])
    out += hl(25.5, 31.5, 2.4, 4.5, op=0.6) + hl(40.5, 31.5, 2.4, 4.5, op=0.6)
    return out


@icon("usage")
def _():
    out = chunk("grape", [rect(5, 5, 17, 35, 5)])
    out += chunk("coral", [rect(28, 5, 15, 14, 5)])
    out += chunk("sun", [rect(28, 25.5, 15, 14.5, 5)])
    out += hl(8.5, 9, 3, 9, op=0.55) + hl(31.5, 8.5, 6, 2.5, op=0.55) + hl(31.5, 29, 6, 2.5, op=0.6)
    return out


@icon("history")
def _():
    t = "blue"
    cx, cy, r = 25, 25, 14.5
    a0, a1 = math.radians(205), math.radians(160 + 360)  # clockwise from a0 around to a1
    p0 = (cx + r*math.cos(a0), cy + r*math.sin(a0))
    p1 = (cx + r*math.cos(a1), cy + r*math.sin(a1))
    arc = f"M{P(*p0)} A{r} {r} 0 1 1 {P(*p1)}"
    # arrowhead at p0 pointing counter-clockwise (tangent = (sin, -cos))
    tx, ty = math.sin(a0), -math.cos(a0)
    nx, ny = -ty, tx
    L = 6
    h1 = (p0[0] - L*tx + L*nx*0.0 + L*0.9*(-tx + nx)*0, 0)
    # arms: rotate back-direction (-t) by +-45deg
    bx, by = -tx, -ty
    def rot(x, y, a): return (x*math.cos(a) - y*math.sin(a), x*math.sin(a) + y*math.cos(a))
    tip = (p0[0] + tx*4, p0[1] + ty*4)
    arm1 = rot(bx, by, math.radians(50)); arm2 = rot(bx, by, math.radians(-50))
    head = f"M{P(tip[0]+arm1[0]*L, tip[1]+arm1[1]*L)} L{P(*tip)} L{P(tip[0]+arm2[0]*L, tip[1]+arm2[1]*L)}"
    out = circ(cx, cy, 9, S(t)).svg(S(t))
    tri = f"M{P(tip[0]+arm1[0]*L*1.2, tip[1]+arm1[1]*L*1.2)} L{P(*tip)} L{P(tip[0]+arm2[0]*L*1.2, tip[1]+arm2[1]*L*1.2)} Z"
    out += chunk(t, [Ln(arc, 5), path(tri, pad=1.5)])
    out += line(f"M{cx} {cy-5} V{cy} L{cx+4} {cy+2.5}", E(t), 3.2)
    return out


@icon("telescope")
def _():
    t = "grape"
    tf = "rotate(-28 24 20)"
    out = chunk(t, [Ln("M24 25 L15 42 M24 25 L33 42", 3.5, S(t))], shade=False)
    out += chunk(t, [rect(10, 14, 26, 11, 5), rect(4, 16, 7, 7, 2.5), rect(35, 12, 8, 15, 3)], transform=tf)
    out += hl(14, 16.5, 9, 2.5, tf, 0.55)
    out += chunk("sun", [path(star4(40, 36, 5.5, 0.32), pad=0.5)], shade=False)
    return out


@icon("wrench")
def _():
    t = "sky"
    tf = "rotate(45 24 24)"
    head = path("M27.5 3.63 V12 H20.5 V3.63 A10 10 0 1 0 27.5 3.63 Z")
    handle = rect(20, 20, 8, 25, 4)
    out = chunk(t, [head, handle], transform=tf)
    out += circ(24, 38, 2).svg(S(t)).replace("/>", f' transform="{tf}"/>')
    out += hl(17, 12, 2.6, 6, tf, 0.55)
    return out

# ---------------------------------------------------------------- moods

def cat_head(extra=""):
    t = "grape"
    face = [path("M9 21 L10.5 5.5 L22 14 Z", pad=2), path("M39 21 L37.5 5.5 L26 14 Z", pad=2),
            ell(24, 28.5, 16, 13.5)]
    out = chunk(t, face, fill=S(t))
    out += path("M12.5 15 L13 10 L17 13 Z", pad=1).svg("#FFB3C0", "#FFB3C0", 2)
    out += path("M35.5 15 L35 10 L31 13 Z", pad=1).svg("#FFB3C0", "#FFB3C0", 2)
    out += path("M22.3 27.6 H25.7 L24 29.6 Z", pad=0.8).svg(B("coral"), B("coral"), 1.6)
    return out


def cheeks():
    return (f'<ellipse cx="13" cy="31.5" rx="3" ry="2" fill="{B("coral")}" opacity="0.4"/>'
            f'<ellipse cx="35" cy="31.5" rx="3" ry="2" fill="{B("coral")}" opacity="0.4"/>')


@icon("oops")
def _():
    e = E("grape")
    out = cat_head() + cheeks()
    out += f'<circle cx="17" cy="23.5" r="2.6" fill="{e}"/><circle cx="31" cy="23.5" r="2.6" fill="{e}"/>'
    out += rect(17, 31.5, 14, 6.5, 3).svg(W, e, 2.6)
    out += line("M17.5 34.75 H30.5 M21.7 32 V37.5 M26.3 32 V37.5", e, 1.6)
    out += chunk("sky", [path("M41.5 21 C43.5 24 44.5 25.5 44.5 27 A3 3 0 0 1 38.5 27 C38.5 25.5 39.5 24 41.5 21 Z")], shade=False)
    return out


@icon("relief")
def _():
    e = E("grape")
    out = cat_head() + cheeks()
    out += line("M13.5 23 Q17 26.5 20.5 23 M27.5 23 Q31 26.5 34.5 23", e, 3)
    out += line("M19.5 31.5 Q21.75 34.5 24 31.5 Q26.25 34.5 28.5 31.5", e, 2.6)
    return out


@icon("dizzy")
def _():
    e = E("grape")
    out = cat_head() + cheeks()
    for cx in (17, 31):
        out += line(f"M{cx} 24 m-1.2 0 a1.2 1.2 0 1 1 2.4 0 a2.6 2.6 0 1 1 -5.2 0 a3.9 3.9 0 1 1 7.8 0", e, 2.2)
    out += line("M18 34 Q21 31.5 24 34 Q27 36.5 30 34", e, 2.6)
    out += chunk("sun", [path(star4(7, 6, 4.5, 0.32), pad=0.3)], shade=False)
    out += chunk("sun", [path(star4(42.5, 19, 3.5, 0.32), pad=0.3)], shade=False)
    return out


HEAD = ('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 48 48" width="48" height="48">'
        '<g stroke-linecap="round" stroke-linejoin="round">')
for name, f in ICONS.items():
    body = f()
    with open(os.path.join(OUT, f"{name}.svg"), "w") as fh:
        fh.write(HEAD + body + "</g></svg>\n")
print(len(ICONS), "icons ->", OUT)
print(" ".join(ICONS))
