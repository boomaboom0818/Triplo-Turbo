"""Build the review and interval chart from completed frozen-binary validation runs."""
import json
import math
from validation_breadth import OUT, FROZEN

NAMES = {'short_pressure':'20 BB · shove / pressure','reference_full_menu':'100 BB · full sizing',
         'reference_common_menu':'100 BB · common sizing','empirical_archive_mix':'Live-like depths · archive mix',
         'deep_pressure':'400 BB · pressure','very_deep_mix':'1,000 BB · archive mix',
         'carry_mixed':'240-hand sessions · archive mix','carry_adaptive':'240-hand sessions · adaptive mix'}


def load(name):
    r=json.loads((OUT/(name+'.json')).read_text());assert r['binary_sha256']==FROZEN;return r


def fmt(x):return f'{x:+.1f}'


def chart(cases):
    lo=min(0,*[r['paired_delta_family_ci95'][0] for r in cases]);hi=max(0,*[r['paired_delta_family_ci95'][1] for r in cases]);pad=(hi-lo)*.03;lo-=pad;hi+=pad
    def x(v):return 100*(v-lo)/(hi-lo)
    lines=[]
    for r in cases:
        l,h=r['paired_delta_family_ci95'];m=r['paired_delta_bb100']
        lines.append(f'<section><span>{NAMES[r["case"]]}</span><div class="axis"><i style="left:{x(0):.2f}%"></i><b style="left:{x(l):.2f}%;width:{x(h)-x(l):.2f}%"></b><em style="left:{x(m):.2f}%"></em></div><strong>{fmt(m)}</strong></section>')
    return '''<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><style>
body{margin:0;font-family:var(--font-sans,system-ui);color:var(--foreground,inherit)}h3{font-size:16px;margin:4px 0}p{font-size:12px;color:var(--muted-foreground,inherit);margin:5px 0 10px}section{display:grid;grid-template-columns:minmax(110px,1fr) minmax(90px,1.8fr) 55px;gap:12px;align-items:center;padding:7px 0;border-top:1px solid var(--border,#8886);font-size:13px}.axis{position:relative;height:20px}.axis:before{content:"";position:absolute;top:10px;width:100%;border-top:1px solid var(--border,#8886)}i{position:absolute;height:20px;border-left:1px dashed var(--muted-foreground,gray)}b{position:absolute;top:9px;border-top:3px solid var(--chart-2,teal)}em{position:absolute;top:5px;width:10px;height:10px;border-radius:100%;background:var(--chart-2,teal);transform:translateX(-50%)}strong{text-align:right;font-variant-numeric:tabular-nums}
</style><h3>Turbo minus the old model, offline</h3><p>Net BB/100 advantage · line = simultaneous 95% interval · dashed line = parity</p>'''+''.join(lines)+'</html>\n'


def main():
    breadth=load('breadth')['trials'];cases=load('challenge')['cases'];river=load('river');recovery=load('recovery')
    rules=json.loads((OUT/'rules.json').read_text());terminal=load('terminal');configured=load('configured')
    assert len(breadth)==30 and len(cases)==8 and {r['case'] for r in cases}==set(NAMES)
    for r in cases:
        estimate=r['paired_delta_bb100'];cl,ch=r['paired_delta_bb100_ci95'];fl,fh=r['paired_delta_family_ci95']
        assert math.isclose(estimate,r['candidate_bb100']-r['control_bb100'],rel_tol=1e-10,abs_tol=1e-9)
        assert fl<=cl<=estimate<=ch<=fh and r['candidate_hands']==r['control_hands']
        assert math.isclose(estimate,sum(r['paired_delta_first_second_half'])/2,rel_tol=1e-10,abs_tol=1e-9)
    native=sum(r['hands'] for r in breadth);candidate=sum(r['candidate_hands'] for r in cases);control=sum(r['control_hands'] for r in cases)
    assert (native,candidate,control)==(1020000,360000,360000)
    primary=next(r for r in cases if r['case']=='empirical_archive_mix'); strong=[r for r in cases if r['paired_delta_family_ci95'][1]<-5]
    positive=[r for r in cases if r['paired_delta_family_ci95'][0]>0];sessions=[r for r in cases if r['unit']=='session'];risk=[]
    extra=configured['candidate_hands'];assert extra==configured['control_hands']==60000
    md=['# Extended evaluation: Turbo profile 20', '', '**Deployment: incumbent live; Turbo offline.**', '',
        f'Played **{native+candidate+extra:,} additional candidate hands** and **{control+extra:,} matched incumbent control hands**. The {native:,} native-arena hands test table-size and seed sensitivity. The {candidate:,} prespecified independent-engine hands and {extra:,} supplementary deployment-configuration hands compare strength and session risk. Every result uses the same frozen executable.', '',
        '| Condition | Turbo hands | Turbo BB/100 | Incumbent BB/100 | Paired difference [simultaneous 95% interval] |',
        '|---|---:|---:|---:|---:|']
    for r in cases:
        l,h=r['paired_delta_family_ci95'];md.append(f"| {NAMES[r['case']]} | {r['candidate_hands']:,} | {fmt(r['candidate_bb100'])} | {fmt(r['control_bb100'])} | {fmt(r['paired_delta_bb100'])} [{fmt(l)}, {fmt(h)}] |")
    md+=['','## Session downside','', 'Each distribution covers 300 **independent sessions**, 240 consecutive hands per session. Drawdown is measured on cumulative table profit, excluding rebuy grants. The two paths share the exogenous deals; bets, subsequent stack sizes and opponent responses can diverge.','',
        '| Field and player | Losing sessions | Sessions with a bust | Total busts | P90 / largest drawdown (BB) |', '|---|---:|---:|---:|---:|']
    for r in sessions:
        for name in ['candidate','control']:
            q=r[name+'_session_risk'];md.append(f"| {r['case']} · {name} | {q['losing_pct']:.1f}% | {q['any_bust_pct']:.1f}% | {q['busts']} | {q['p90_drawdown_bb']:.1f} / {q['max_drawdown_bb']:.1f} |")
        a=r['candidate_session_risk'];b=r['control_session_risk'];
        if a['any_bust_pct']>b['any_bust_pct']+10 or a['p90_drawdown_bb']>b['p90_drawdown_bb']*1.5:risk.append(r['case'])
    a,b=configured['candidate'],configured['control'];l,h=configured['paired_delta_ci95']
    md+=['','## Supplement: deployed buy-in and incumbent opponent modeling','',f"Actual restore settings: 50 BB buy-in, base-guard checkpoint, minimum 64 visits, equity-guarded-v3, opponent statistics on. Across 200 independent 300-hand sessions at 50 BB start/re-entry, Turbo earned **{fmt(a['bb100'])} BB/100** vs the incumbent control's **{fmt(b['bb100'])}**, paired delta **{fmt(configured['paired_delta_bb100'])}** (pointwise 95% session interval [{fmt(l)}, {fmt(h)}]). Bust-session rates: {a['any_bust_pct']:.1f}% vs {b['any_bust_pct']:.1f}%; 90th-percentile drawdowns: {a['p90_drawdown_bb']:.1f} vs {b['p90_drawdown_bb']:.1f} BB. This separate estimate is not pooled with the eight prespecified comparisons. The incumbent decision adapter builds opponent profiles from public actions within each simulated session, not from identifiers or historical profiles on the real account."]
    pos=sum(r['hero_bb_per_100']>0 for r in breadth);cal=river['equity_uniform_no_wager'];v=sum(river['violations'].values())
    clean,dupes,sparse=recovery['cases'];rc=rules['counters'];bad_rules=sum(rules['discrepancies'].values())
    md+=['','## Interpretation','',f"- The primary live-depth mixed field returned **{fmt(primary['paired_delta_bb100'])} BB/100 relative to the incumbent**. {len(positive)} of 8 paired intervals lie entirely above zero. Material downside screens (whole interval below −5): {', '.join(r['case'] for r in strong) or 'none'}.",
        f"- Session risk flags on the predeclared thresholds: {', '.join(risk) or 'none'}. Losing individual sessions remains possible even in a positive-return field.",
        f'- The native arena had a positive point estimate in {pos} of 30 runs (2–6 seats, 3 disjoint seeds per condition). The cheap scripted opponents can be very weak. These returns were not pooled with checkpoint results.',
        f"- River oracle: {river['counts']['decisions']:,} decisions, including {river['counts']['nut_call_tests']:,} all-in calls with an independently established unbeatable hand; **{v} invariant violations**. For uniform heads-up ranges, equity-estimate RMSE was {cal['rmse']*100:.2f} percentage points. Oracle probes are not counted as hands.",
        f"- Simulator audit (PokerKit {rules['pokerkit']}): {rc['settlements_matched']:,}/{rules['hands']:,} complete trajectories and payouts matched. {rc['off_menu_raises']:,} arbitrary raises outside the discrete menu were exercised; first-discrepancy categories: {rules['discrepancies'] or 'none'}. These are rules cases, not profit samples.",
        f"- Recorded-traffic replay: {clean['legal_decisions']:,} decisions on the clean trace; {dupes['duplicated_or_late']:,} duplicate/late frames in one pass and {sparse['dropped_public_actions']:,} removed public actions in another. Matching decision counts: {recovery['matching_decision_counts']}; missing views: {[r['missing_view'] for r in recovery['cases']]}. This is reducer-level evidence, not proof that different histories lead to identical strategies.",
        f"- Historical terminal decisions: {terminal['counts']['confirmed_terminal_results']} confirmed heads-up call/fold spots reached by the old live policies. Turbo would fold {terminal['incumbent_to_candidate'].get('call_to_fold',0)} actual calls (realized difference: {fmt(terminal['known_fold_instead_of_call_net_bb'])} BB total) and call {terminal['incumbent_to_candidate'].get('fold_to_call',0)} actual folds (unknown returns). The overall logical bounds in this selected subset are {terminal['identified_and_bounded_total_delta_bb']} BB. This does not identify a live win rate; it suggests a targeted review of expensive folds.",
        '- All intervals are approximate. The challenge chart/table adjusts for eight comparisons using independent deal blocks or sessions. Static checkpoints, a stylized adaptive opponent and unbounded virtual-chip re-entries do not reproduce the entire public pool. These tests do not establish live profitability or authorize a deployment.', '',
        'Data: `breadth.json`, `challenge.json`, `configured.json`, `river.json`, `rules.json`, `recovery.json`, `terminal.json`. The challenge file also contains pointwise intervals, the two half-sample estimates per condition, the number of common-menu size translations, and counts of adaptive responses. See `README.md` for the trial design.']
    verdict=[]
    if primary['paired_delta_family_ci95'][0]<=0:verdict.append('the primary mixed field does not establish an advantage')
    if strong:verdict.append('one or more stress fields show a material shortfall')
    if risk:verdict.append('a session downside threshold was exceeded')
    if configured['paired_delta_ci95'][1]<-5:verdict.append('the supplementary deployment-configuration test flags a material regression')
    if bad_rules or v or not recovery['matching_decision_counts']:verdict.append('a correctness or simulation discrepancy needs investigation')
    caution=[]
    negative=[NAMES[r['case']] for r in cases if r['paired_delta_family_ci95'][1]<0]
    if negative:caution.append('the incumbent beats Turbo under '+', '.join(negative))
    if configured['paired_delta_ci95'][0]<=0<=configured['paired_delta_ci95'][1]:caution.append('the 50 BB deployment-configuration test does not establish a Turbo advantage')
    screen='; '.join(verdict) if verdict else 'the prespecified material-loss and downside thresholds were not breached'
    assessment='**Recommendation: keep the incumbent live.** '+screen[0].upper()+screen[1:]+'.'
    if caution:assessment+=' However, '+'; '.join(caution)+'.'
    md[3:3]=['',assessment]
    (OUT/'RESULTS.md').write_text('\n'.join(md)+'\n');(OUT/'intervals.html').write_text(chart(cases))
    print('\n'.join(md))


if __name__=='__main__':main()
