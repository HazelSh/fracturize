#!/usr/bin/env python3
"""Does piece overlap predict fog?

The hypothesis §4.3 of FIT-OPTIMIZER-PLAN.md rests on: fog is maps stacking.
If that is right, overlap should track the fog indicators -- higher overlap
going with lower lace and higher coverage -- and a penalty on overlap is a
mechanism-level fix. If overlap turns out to be uncorrelated with how the fit
looks, the rationale is wrong and the penalty is not worth building.

Reported within each condition as well as pooled, because a pooled correlation
across conditions can be manufactured entirely by both quantities trending with
the perceptual ratio.
"""
import re
import sys

import numpy as np
from PIL import Image

sys.path.insert(0, 'tools')
from lace_score import lace, coverage, cells  # noqa: E402

TAGS = ['0.0', '0.4', '1.0']
CELL = 480


def small(im):
    g = np.asarray(im.convert('L').resize((32, 32), Image.LANCZOS), float) / 255.
    return (g - g.mean()) / (g.std() + 1e-9)


def rows(log_text, tag):
    """seed -> (loss, dimension, overlap) from one condition's table."""
    block = log_text.split(f'=== perceptual {tag} ===')[1].split('===')[0]
    out = {}
    for m in re.finditer(
            r'cell\s+(\d+)\s+seed\s+(\d+): loss ([\d.]+) \(from [\d.]+, [\d.]+× better\)\s+'
            r'dimension (\S+)\s+overlap (\d+)%', block):
        cell, seed, loss, dim, ov = m.groups()
        out[int(seed)] = dict(cell=int(cell) - 1, loss=float(loss),
                              dim=(None if dim == '--' else float(dim)),
                              overlap=int(ov) / 100.0)
    return out


log = open(sys.argv[1]).read()
root = sys.argv[2]

target = small(Image.open(f'{root}/o0.0.png').crop((0, 0, CELL, CELL)))
pool = {k: [] for k in ('ov', 'lace', 'cov', 'like', 'loss')}

def corr(a, b):
    a, b = np.asarray(a), np.asarray(b)
    if len(a) < 3 or a.std() < 1e-9 or b.std() < 1e-9:
        return float('nan')
    return float(np.corrcoef(a, b)[0, 1])

print(f'{"perc":>5} {"overlap mean":>13} {"range":>15} '
      f'{"r(ov,lace)":>11} {"r(ov,cov)":>10} {"r(ov,like)":>11} {"r(ov,loss)":>11}')
print('-' * 82)

for tag in TAGS:
    tab = rows(log, tag)
    sheet_cells = {i: c for i, c in cells(f'{root}/o{tag}.png', CELL, None)}
    ov, lc, cv, lk, ls = [], [], [], [], []
    for seed, r in sorted(tab.items()):
        c = sheet_cells.get(r['cell'])
        if c is None:
            continue
        ov.append(r['overlap']); ls.append(r['loss'])
        lc.append(lace(c)); cv.append(coverage(c))
        lk.append(float((small(c) * target).mean()))
    for k, v in zip(('ov', 'lace', 'cov', 'like', 'loss'), (ov, lc, cv, lk, ls)):
        pool[k] += v
    a = np.array(ov)
    print(f'{tag:>5} {a.mean():>13.3f} {f"{a.min():.2f}-{a.max():.2f}":>15} '
          f'{corr(ov, lc):>11.3f} {corr(ov, cv):>10.3f} '
          f'{corr(ov, lk):>11.3f} {corr(ov, ls):>11.3f}')

n = len(pool['ov'])
print(f'\npooled over all {n} fits:')
for name, key in (('lace', 'lace'), ('coverage', 'cov'),
                  ('likeness', 'like'), ('loss', 'loss')):
    r = corr(pool['ov'], pool[key])
    # Fisher z for a CI, so "no relationship" is distinguishable from
    # "underpowered", which is the distinction this whole test turns on.
    z = np.arctanh(r); se = 1 / np.sqrt(n - 3)
    lo, hi = np.tanh(z - 1.96 * se), np.tanh(z + 1.96 * se)
    verdict = 'no relationship' if lo < 0 < hi else 'REAL'
    print(f'  r(overlap, {name:>8}) = {r:+.3f}  95% CI [{lo:+.3f},{hi:+.3f}]  {verdict}')
