"""Render independent match results with point estimates and block intervals."""
import argparse
import html
import json
import math
from pathlib import Path

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('results',type=Path,nargs='+');p.add_argument('--out',type=Path,required=True)
a=p.parse_args();rows=[]
for path in a.results:
    for r in json.loads(path.read_text()):
        delta='paired_delta_bb_per_100';k=delta if delta in r else 'bb_per_100'
        group='3 trained + 2 rule bots' if 'paired champion' in r['opponents'] else ('5 unseen checkpoint copies' if r.get('fallback')=='equity-v1' else '5 trained reference copies')
        label='Live-like stacks' if r['stack_bb']=='empirical' else 'Unequal stacks' if r['stack_bb']=='mixed' else f"{r['stack_bb']} BB stacks"
        rows.append((group,label,r['hands'],r[k],*r[k+'_block_95pct_ci']))
lo=math.floor(min(0,min(x[-2] for x in rows))/10)*10
hi=math.ceil(max(0,max(x[-1] for x in rows))/10)*10
x=lambda v:100*(v-lo)/(hi-lo or 1)
items=[]
for group,label,n,estimate,l,h in rows:
    items.append(f'''<section><div><b>{html.escape(label)}</b><small>{html.escape(group)} · {n:,} hands</small></div>
    <div class="axis"><i style="left:{x(0):.2f}%"></i><em style="left:{x(l):.2f}%;width:{x(h)-x(l):.2f}%"></em><strong style="left:{x(estimate):.2f}%"></strong></div>
    <div class="num">{estimate:+.1f}<small>[{l:+.1f}, {h:+.1f}]</small></div></section>''')
a.out.write_text('''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>
body{margin:0;font-family:var(--font-sans,system-ui);color:var(--foreground,inherit)}h3{font-size:16px;margin:4px 0}p,small{font-size:12px;color:var(--muted-foreground,inherit)}p{margin:4px 0 12px}section{display:grid;grid-template-columns:minmax(110px,1fr) minmax(90px,2fr) minmax(82px,auto);gap:12px;align-items:center;padding:7px 0;border-top:1px solid var(--border,#8886);font-size:13px}small{display:block;margin:2px 0}.axis{position:relative;height:32px}.axis:before{content:"";position:absolute;top:16px;width:100%;border-top:1px solid var(--border,#8886)}i{position:absolute;height:32px;border-left:1px dashed var(--muted-foreground,gray)}em{position:absolute;top:15px;border-top:3px solid var(--chart-2,teal)}strong{position:absolute;top:11px;width:10px;height:10px;border-radius:100%;background:var(--chart-2,teal);transform:translateX(-50%)}.num{text-align:right;font-variant-numeric:tabular-nums}b{font-weight:600}
</style><h3>Turbo’s edge over the trained reference</h3><p>Net BB/100 difference · dot = estimate, line = approximate 95% interval · dashed line = break-even</p>'''+''.join(items)+'</html>\n')
