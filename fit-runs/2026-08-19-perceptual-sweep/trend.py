"""Is the lace-up / likeness-down trade-off real, or n=20 noise?

Every condition ran the same 20 seeds, so this is a within-seed trend test:
fit a slope of (metric vs perceptual) for each seed separately, then ask
whether the 20 slopes have a non-zero mean. Pairing this way removes the seed
variance that swamped every pairwise comparison.
"""
import re, sys
import numpy as np
from PIL import Image
sys.path.insert(0, 'tools')
from lace_score import lace, cells

VALUES = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0]
CELL = 480
root, logp = sys.argv[1], sys.argv[2]
log = open(logp).read()

def small(im):
    g = np.asarray(im.convert('L').resize((32, 32), Image.LANCZOS), float) / 255.
    return (g - g.mean()) / (g.std() + 1e-9)

target = small(Image.open(f'{root}/p0.0.png').crop((0, 0, CELL, CELL)))

L, K = {}, {}          # [perceptual][seed] -> lace / likeness
for v in VALUES:
    tag = f'{v:.1f}'
    block = log.split(f'=== perceptual {tag} ===')[1].split('===')[0]
    idx2seed = {int(c) - 1: int(s)
                for c, s in re.findall(r'cell\s+(\d+)\s+seed\s+(\d+):', block)}
    L[v], K[v] = {}, {}
    for i, c in cells(f'{root}/p{tag}.png', CELL, None):
        if i in idx2seed:
            L[v][idx2seed[i]] = lace(c)
            K[v][idx2seed[i]] = float((small(c) * target).mean())

seeds = sorted(set.intersection(*[set(L[v]) for v in VALUES]))
x = np.array(VALUES); xc = x - x.mean()

def trend(D, name):
    slopes = np.array([
        float(np.sum(xc * (np.array([D[v][s] for v in VALUES])
                           - np.mean([D[v][s] for v in VALUES]))) / np.sum(xc**2))
        for s in seeds])
    se = slopes.std(ddof=1) / np.sqrt(len(slopes))
    t = slopes.mean() / se
    print(f'{name:>10}: slope per unit perceptual {slopes.mean():+.4f} '
          f'(se {se:.4f})  t={t:+.2f} on {len(slopes)-1} df  '
          f'{"SIGNIFICANT" if abs(t) > 2.09 else "not significant"}  '
          f'({int((slopes>0).sum())}/{len(slopes)} seeds positive)')
    return slopes

print(f'within-seed trend across perceptual 0.0..1.0, n={len(seeds)} seeds\n')
trend(L, 'lace')
trend(K, 'likeness')

print('\nmarginal trade per step (mean over seeds):')
print(f'{"step":>10} {"d lace":>9} {"d likeness":>12}   verdict')
for a, b in zip(VALUES, VALUES[1:]):
    dl = np.mean([L[b][s] - L[a][s] for s in seeds])
    dk = np.mean([K[b][s] - K[a][s] for s in seeds])
    v = 'lace nearly free' if dl > 0 and dk > -0.006 else (
        'bad trade' if dl <= 0.02 else 'lace bought with likeness')
    print(f'{a:.1f}->{b:.1f} {dl:>+9.4f} {dk:>+12.4f}   {v}')
