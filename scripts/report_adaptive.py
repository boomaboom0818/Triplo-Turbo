"""Summarize the frozen three-way confirmation in the repository and chat."""
import json
from pathlib import Path

ROOT=Path(__file__).resolve().parents[1];OUT=ROOT/'benchmarks/adaptation'
NAMES={'reactive100':'Reactive · 100 BB','configured50':'Configured · 50 BB','mixed':'Mixed · live-like depths','static100':'Static · 100 BB','elastic100':'Unseen price-sensitive · 100 BB'}


def generate():
    result=json.loads((OUT/'confirm.json').read_text());fit=json.loads((OUT/'fit.json').read_text());diagnosis=json.loads((OUT/'diagnose.json').read_text())
    rows=result['fields'];assert set(rows)==set(NAMES) and all(r['sessions']==300 for r in rows.values())
    hands=sum(r['hands_per_branch'] for r in rows.values());assert hands==378000
    names={21:'Candidate',20:'Frozen',0:'Incumbent'}
    md=['# Profile 21: does the preflop change pay?', '', '**Offline only.** The old model remains the public player. See the [trial design](README.md).', '',
        f'Confirmation: **{hands:,} hands per policy**, {3*hands:,} in total. Each comparison pairs complete sessions on shared exogenous deals. The candidate was frozen before this seed range.', '',
        '| Field | Candidate | Frozen 20 | Incumbent | Δ vs 20 [95%] | Δ vs incumbent [95%] |', '|---|---:|---:|---:|---:|---:|']
    def entry(d,which='ci95'):
        l,h=d[which];return f"{d['delta']:+.1f} [{l:+.1f}, {h:+.1f}]"
    for k,r in rows.items():
        b=r['branches'];p=r['paired'];assert all(abs(b['21']['bb100']-b[t]['bb100']-p[t]['delta'])<1e-8 for t in ['20','0'])
        md.append(f"| {NAMES[k]} | {b['21']['bb100']:+.1f} | {b['20']['bb100']:+.1f} | {b['0']['bb100']:+.1f} | {entry(p['20'])} | {entry(p['0'])} |")
    md+=['','All values above are **net BB/100**. The intervals above are pointwise. To read all ten comparisons together, use the simultaneous 95% intervals below (Bonferroni normal approximation, session-clustered). An interval containing zero does not resolve which policy is better.', '',
        '| Field | Δ vs frozen [family 95%] | Δ vs incumbent [family 95%] |','|---|---:|---:|']
    for k,r in rows.items():md.append(f"| {NAMES[k]} | {entry(r['paired']['20'],'family95')} | {entry(r['paired']['0'],'family95')} |")
    md+=['','## Downside and behavior','', '90th-percentile peak-to-trough drawdown is in big blinds over a 240-hand session (300 hands at 50 BB). Bust percentage is the share of sessions with at least one re-entry.', '',
        '| Field | Player | P90 drawdown | Bust sessions | Won | Preflop raises per 100 hands¹ |', '|---|---|---:|---:|---:|---:|']
    for k,r in rows.items():
        for b in [21,20,0]:
            x=r['branches'][str(b)];e=x['events'];n=sum(e.get('all:'+s,0) for s in ['foldthrough','call','reraise']);md.append(f"| {NAMES[k]} | {names[b]} | {x['p90_dd']:.1f} | {x['bust_session_pct']:.1f}% | {x['win_pct']:.1f}% | {100*n/r['hands_per_branch']:.1f} |")
    md+=['','¹ Counts hands with at least one hero raise; never more than one per hand. Percent won is descriptive, not the selection objective.','','## Diagnosis and fit','',
         '| Field | Weak opens | Fold-through | Reraised | Net BB per weak open |', '|---|---:|---:|---:|---:|']
    for k,r in diagnosis['fields'].items():
        e=r['branches']['20']['events'];n=sum(e.get('weak_open:'+s,0) for s in ['foldthrough','call','reraise']);md.append(f"| {NAMES[k]} | {n:,} | {100*e['weak_open:foldthrough']/n:.1f}% | {100*e['weak_open:reraise']/n:.1f}% | {e['weak_open:net_bb']/n:+.2f} |")
    md+=['', f"Fit: {fit['hands']:,} baseline training hands, {fit['forks']:,} full-hand alternative trajectories, {len(fit['choices'])} enabled buckets. The first simple tighten-up trial did not improve on profile 20 in the target 100 BB field. The fitted candidate chooses among check/fold, check/call and capped sizes using public evidence; a bucket without sufficient training or recent support retains the original decision. When the alternative changes the action it consumes the same once-per-hand response budget.",'',
        'Actual changes on the held-out sessions:', '', '| Field | Check/fold | Cheap call | Resized raise |','|---|---:|---:|---:|']
    for k,r in rows.items():
        e=r['branches']['21']['events'];md.append(f"| {NAMES[k]} | {e.get('defended_pass',0):,} | {e.get('defended_limp',0):,} | {e.get('defended_resize',0):,} |")
    md+=['',f"Candidate binary: `{result['candidate']}`. Frozen comparator: `{result['frozen']}`.", '', 'These are simulated fields and table-local opponent histories. They measure sample uncertainty within these matchups, not uncertainty about the makeup of a real public table. The old agent’s cross-session real-player profiles are not imported.']
    (OUT/'RESULTS.md').write_text('\n'.join(md)+'\n')
    series=[p for r in rows.values() for p in r['paired'].values()];lo=min(0,min(x['ci95'][0] for x in series))-5;hi=max(0,max(x['ci95'][1] for x in series))+5
    def pct(x):return 100*(x-lo)/(hi-lo)
    html=['''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>body{font:14px system-ui;color:var(--foreground,inherit);margin:4px}h3{margin:3px 0}p{margin:4px 0 14px;font-size:12px}section{display:grid;grid-template-columns:minmax(112px,1.2fr) 2fr 3.5em;gap:10px;align-items:center;padding:4px 0}.axis{position:relative;height:16px;background:linear-gradient(transparent 48%,#8886 48%,#8886 54%,transparent 54%)}i,b,em{position:absolute}i{height:16px;border-left:1px dashed gray}b{top:7px;border-top:3px solid currentColor}em{top:4px;width:9px;height:9px;border-radius:50%;background:currentColor;transform:translateX(-50%)}strong{text-align:right}.a{color:var(--chart-2,#20a39c)}.b{color:var(--chart-1,#be9351)}</style><h3>What changed in profit per 100 hands?</h3><p>Candidate minus <span class="a">frozen Turbo</span> / <span class="b">incumbent</span>. Dot: paired estimate. Line: pointwise 95% interval. Dashed: no difference.</p>''']
    for k,r in rows.items():
        for b,color in [('20','a'),('0','b')]:
            d=r['paired'][b];l,h=d['ci95'];m=d['delta'];html.append(f'<section><span>{NAMES[k] if b=="20" else ""}</span><div class="axis {color}"><i style="left:{pct(0):.2f}%"></i><b style="left:{pct(l):.2f}%;width:{pct(h)-pct(l):.2f}%"></b><em style="left:{pct(m):.2f}%"></em></div><strong class="{color}">{m:+.1f}</strong></section>')
    (OUT/'intervals.html').write_text(''.join(html)+'</html>\n')


if __name__=='__main__':generate()
