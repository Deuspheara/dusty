#!/usr/bin/env python3
import os, sys
src, out = sys.argv[1], sys.argv[2]
names = sorted(f[:-4] for f in os.listdir(src) if f.endswith(".svg"))
order = ["build", "cache-dev", "cache-app", "junk", "docker", "redownload",
         "reveal", "never", "trash", "put-back", "search", "settings", "back", "check", "warning", "lock",
         "practice", "sparkle", "paw", "gift", "party", "broom", "schedule",
         "uninstall", "big-files", "usage", "history", "telescope", "wrench", "oops", "relief", "dizzy"]
names = [n for n in order if n in names] + [n for n in names if n not in order]
big = "--big" in sys.argv
sizes = [20, 32, 48] + ([96] if big else [])
cells = []
for n in names:
    def imgs():
        return "".join(f'<img src="file://{src}/{n}.svg" width="{s}" height="{s}">' for s in sizes)
    cells.append(f'<div class="cell"><div class="name">{n}</div><div class="pair">'
                 f'<div class="card light">{imgs()}</div><div class="card dark">{imgs()}</div></div></div>')
cols = 3
html = f"""<!doctype html><html><head><meta charset="utf-8"><style>
body{{margin:0;padding:24px;background:linear-gradient(90deg,#FFFBF5 50%,#17112B 50%);font:600 13px -apple-system,system-ui;}}
h1{{font:900 22px ui-rounded,-apple-system;color:#2B2340;margin:0 0 16px}}
.grid{{display:grid;grid-template-columns:repeat({cols},1fr);gap:14px}}
.cell{{background:#FFFBF5;border-radius:14px;padding:8px;border:1px solid #E8E4F3}}
.name{{color:#2B2340;margin:0 0 6px 4px}}
.pair{{display:flex;gap:6px}}
.card{{flex:1;display:flex;align-items:center;justify-content:space-around;gap:6px;padding:8px;border-radius:12px}}
.light{{background:#FFFFFF;border:1px solid #E8E4F3}}
.dark{{background:#251C44;border:1px solid #3A2F63}}
</style></head><body><h1>Dusty UI icons</h1><div class="grid">{''.join(cells)}</div></body></html>"""
open(out, "w").write(html)
print(len(names), "icons in", out)
