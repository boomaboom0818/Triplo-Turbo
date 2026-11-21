"""Render Turbo-only paired session comparisons."""
import argparse
import json
from pathlib import Path

OUT = Path(__file__).resolve().parents[1] / 'benchmarks/guard'
p = argparse.ArgumentParser(description=__doc__)
experiment = p.add_mutually_exclusive_group()
experiment.add_argument('--lab', action='store_true', help='render the lab candidate against live profile 22')
experiment.add_argument('--sticky', action='store_true', help='render the calling-history candidate against live profile 22')
experiment.add_argument('--strategy', action='store_true', help='render the preflop candidate against profile 28')
experiment.add_argument('--context', action='store_true', help='render the opponent-aware call guard against profile 28')
args = p.parse_args()
labels = dict(mixed_50='Mixed · 50 BB', price_sensitive_50='Price-sensitive · 50 BB',
              sticky_50='Calling-heavy · 50 BB', reactive_100='Reactive · 100 BB',
              calling_heavy_50='Calling-heavy · 50 BB', value_pressure_50='Value and pressure · 50 BB',
              value_and_reactive_50='Unseen mixed pressure · 50 BB', counter_50='Counterattacks · 50 BB',
              mixed_price_50='Mixed prices · 50 BB', strong_short_50='Tough unseen · 50 BB',
              counter_deep_100='Deep unseen · 100 BB', turbo_pool_50='Turbo self-play mix · 50 BB')
if args.lab or args.sticky or args.strategy or args.context:
    if args.context:
        OUT = OUT.parent / 'strategy/context'
    else:
        OUT = OUT.parent / 'strategy' if args.strategy else OUT / ('sticky' if args.sticky else 'lab')
    lab = json.loads((OUT / 'confirmation.json').read_text())
    profile = 34 if args.context else 28 if args.sticky else lab['candidate']
    control = 28 if args.strategy or args.context else 22
    rows = [dict(field=r['field'], paired_delta_bb100=(d := r['paired_deltas_bb100'][f'{profile}-{control}'])['mean'],
                 family_ci95=d['family_ci95']) for r in lab['fields']]
    title, comparison = 'Ramsay: fresh-deal confirmation', f'Profile {profile} minus profile {control}'
else:
    rows = json.loads((OUT / 'confirmation.json').read_text())['fields']
    title, comparison = 'Ramsay: profit impact of the postflop guard', 'Profile 22 minus profile 21'
lo = min(0, *[r['family_ci95'][0] for r in rows]) - 3
hi = max(0, *[r['family_ci95'][1] for r in rows]) + 3


def x(v):
    return 100*(v-lo)/(hi-lo)


parts = ['''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>
body{margin:0;font:14px var(--font-sans,system-ui);color:var(--foreground,inherit)}h3{margin:4px 0}p{font-size:12px;margin:4px 0 10px;color:var(--muted-foreground,inherit)}section{display:grid;grid-template-columns:minmax(105px,1fr) minmax(110px,1.8fr) 3.5em;gap:10px;align-items:center;padding:7px 0;border-top:1px solid var(--border,#8886)}.axis{position:relative;height:16px;border-bottom:1px solid var(--border,#8886)}i,b,em{position:absolute}i{height:16px;border-left:1px dashed var(--muted-foreground,gray)}b{top:7px;border-top:3px solid var(--chart-2,teal)}em{top:4px;width:9px;height:9px;border-radius:50%;transform:translateX(-50%);background:var(--chart-2,teal)}strong{text-align:right}
</style>''', f'<h3>{title}</h3><p>{comparison}, net BB/100. Lines: simultaneous 95% intervals. Dashed line: no difference. Offline only.</p>']
for row in rows:
    d = row['paired_delta_bb100']
    l, h = row['family_ci95']
    parts.append(f'<section><span>{labels[row["field"]]}</span><div class="axis">'
                 f'<i style="left:{x(0):.2f}%"></i><b style="left:{x(l):.2f}%;width:{x(h)-x(l):.2f}%"></b>'
                 f'<em style="left:{x(d):.2f}%"></em></div><strong>{d:+.1f}</strong></section>')
(OUT / 'intervals.html').write_text(''.join(parts) + '</html>\n')
