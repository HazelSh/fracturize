"""Coverage per condition: does the attractor still fill the frame?

`lace` cannot tell dense filigree from a nearly-empty cell -- both have huge
local contrast -- so it inverts at the sparse end and rewards degeneracy. This
is the missing axis: what fraction of the cell carries any material at all.

Fog is high coverage and low lace. Filigree is mid coverage and high lace.
A collapsed fit is low coverage and high lace, and only this separates it
from the third case.
"""
import re, sys
import numpy as np
from PIL import Image
sys.path.insert(0, 'tools')
from lace_score import lace, cells, LABEL_SKIP

VALUES = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0]
CELL = 480
root, logp = sys.argv[1], sys.argv[2]
log = open(logp).read()

def coverage(im):
    g = np.asarray(im.convert('L'), float)[LABEL_SKIP:, :] / 255.
    return float((g > 0.02).mean())

print(f'{"perc":>5} {"coverage mean":>14} {"cov min":>9} '
      f'{"cells under 10%":>16} {"anneal gain":>12}')
for v in VALUES:
    tag = f'{v:.1f}'
    block = log.split(f'=== perceptual {tag} ===')[1].split('===')[0]
    gains = [float(g) for g in re.findall(r'([\d.]+)× better', block)]
    cov = np.array([coverage(c) for _, c in cells(f'{root}/p{tag}.png', CELL, None)])
    print(f'{tag:>5} {cov.mean():>14.3f} {cov.min():>9.3f} '
          f'{int((cov < 0.10).sum()):>10}/{len(cov):<5} {np.mean(gains):>11.2f}x')
