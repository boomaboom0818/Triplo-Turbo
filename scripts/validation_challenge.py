"""Frozen-candidate strength and bankroll stress tests. Run in the reference venv.

No authenticated/network calls or model writes. The benchmark uses positions for
observations and physical seats for longitudinal balances. Each control sees the
same chance deals, starting bankroll, lineup, per-seat seeds and button schedule.
"""
import argparse
import gc
import hashlib
import json
import math
import multiprocessing as mp
import random
import statistics as stats
import sys
from pathlib import Path

import benchmark_reference as ref
from validation_breadth import ROOT, BINARY, FROZEN, OUT

BASE = 'runs/bot/pressure-fresh-001/base-guard.json'
ALT = 'runs/bot/history-sustained-001/checkpoint.json'
FILES = {BASE: 'f3ed3b73624edfa43568e1248abe80b61deeb320f217e6d9a123d32879f9b6ca',
         ALT: '4e606ddc39214b413fe7df48f66c3781c1a0413f9d2af74ad6e6a89fe800bcb8'}
LINEUPS = {'reference': ['old']*5,
           'pressure': ['old', 'old', 'alt', 'aggressive-v1', 'shover-v1'],
           'archive_mix': ['old', 'alt', 'alt', 'station-v1', 'aggressive-v1'],
           'adaptive_mix': ['old', 'alt', 'adaptive', 'adaptive', 'station-v1']}
# Same seeds and seat rotations for the free-sizing/common-menu ablation only.
CASES = [('short_pressure', 20, 'pressure', False, 800000000),
         ('reference_full_menu', 100, 'reference', False, 801000000),
         ('reference_common_menu', 100, 'reference', True, 801000000),
         ('empirical_archive_mix', 'empirical', 'archive_mix', False, 802000000),
         ('deep_pressure', 400, 'pressure', False, 803000000),
         ('very_deep_mix', 1000, 'archive_mix', False, 804000000)]
SESSION_CASES = [('carry_mixed', 'empirical', 'archive_mix', 910000000),
                 ('carry_adaptive', 100, 'adaptive_mix', 920000000)]
FAMILY = len(CASES) + len(SESSION_CASES)


def init():
    global alt, rules
    from poker.history_cfr import HistoryPolicy
    from poker.equity_pricing import make_fallback
    from poker.rule_opponents import RulePolicy
    ref.init()
    alt = [HistoryPolicy(alt_saved, min_visits=64) for _ in range(6)]
    for policy in alt:
        policy.fallback = make_fallback('equity-v1')
    rules = {k: RulePolicy(k) for k in ('aggressive-v1','station-v1','tight-v1','shover-v1')}


def lineup(name, seed, hero):
    other = list(LINEUPS[name]); random.Random(seed ^ 0xd5e2).shuffle(other)
    return [(['hero'] + other)[(seat-hero) % 6] for seat in range(6)]


def stakes(depth, seed):
    values = list(empirical[seed % len(empirical)]) if depth == 'empirical' else [depth]*6
    random.Random(seed).shuffle(values)
    return [2*s for s in values]  # reference uses integer half-BB units


def configured_action(g, o, seat, stats_book, counters):
    """Use the incumbent's actual adapter and public-data fit on synthetic seats."""
    from scripts.openpoker_client import decision
    menu=[{'action':'fold'}] if 'fold' in o.legal else []
    menu.append({'action':'call','amount':10*o.call} if o.call else {'action':'check'})
    if g.can_raise():
        top=max(g.bets);maximum=g.bets[o.position]+g.stacks[o.position]
        menu.append(dict(action='raise',min=10*min(maximum,top+max(2,g.raise_size)),max=10*maximum))
    s=dict(type='table_state',hand_id='offline',street=o.street,board=o.board,big_blind=20,pot=10*o.pot,
           dealer_seat=5,actor_seat=o.position,hero=dict(seat=o.position,hole_cards=o.hole,valid_actions=menu,turn_token='offline'),
           seats=[dict(seat=i,name=f'seat-{physical}',in_hand=True,folded=not o.active[i],stack=10*g.stacks[i],bet=10*g.bets[i]) for i,physical in enumerate(g.order)])
    p=ref.field[seat];p.fallback.opponent_stats=stats_book
    history=[(pos,pos,st,a,pot,call) for _,pos,st,a,pot,call in o.history]
    result=decision(s,p,history,history_complete=True);counters['adapter_decisions']+=1
    if o.street>0 and decision.last.get('opponent_profiles'):counters['postflop_with_learned_profiles']+=1
    a=result['action'];return result['amount']//10 if a=='raise' else 'call' if a in ('call','check') else a


def play(seed, hero, button, stacks, roles, challenger, snap_menu=False, tendency=(0,0), stats_book=None, counters=None, profile=20, session_data=None):
    from poker.fast_game import FastGame
    from poker.rule_opponents import chen
    g = FastGame(seed, 6, button=button, stacks=stacks, lean=True)
    for seat in range(6):
        for policies in (ref.field, alt):
            policies[seat].rng = random.Random(f'extended:{seed}:{seat}')
    rng = random.Random(seed*6+hero); events=[]; hero_raised=False; adjusted=0; pressure_replies=0
    hands, opens = tendency
    # Population prior equal to 6 raises in 24 hands. This opponent sees only
    # previous public preflop raises and the current public bet/own holding.
    frequent = hands >= 16 and (opens+6)/(hands+24) > .34
    if session_data: session_data.start(hero)
    while g.live:
        o=g.observe(); seat=g.order[o.position]; role=roles[seat]
        if challenger and seat==hero:
            action,_=ref.hero_decision(g,o,profile,events,rng,session=session_data)
            if snap_menu and isinstance(action,int) and action not in o.legal:
                sizes=[a for a in o.legal if isinstance(a,int)]; assert sizes
                action=min(sizes,key=lambda n:(abs(n-action),n)); adjusted+=1
        elif role in ('hero','old') and stats_book is not None:
            action=configured_action(g,o,seat,stats_book,counters)
        elif role in ('hero','old','alt'):
            policy=(alt if role=='alt' else ref.field)[seat]
            action=policy.act(o,history_complete=True)
        elif role=='elastic':
            # Held-out: no hero-frequency trigger. Elastic callers keep defending
            # small bets; with strong hands they use a small rather than pot reraise.
            action=rules['aggressive-v1'].act(o)
            if hero_raised and o.street==0 and 0<o.call<=12:
                sizes=[a for a in o.legal if isinstance(a,int)]
                if chen(o.hole)>=9 and sizes: action=sizes[0]
                elif chen(o.hole)>=4: action='call'
        elif role=='adaptive':
            action=rules['aggressive-v1'].act(o)
            if frequent and hero_raised and o.street==0 and 0<o.call<=16:
                sizes=[a for a in o.legal if isinstance(a,int)]
                strength=chen(o.hole)
                if strength>=8 and sizes: action=sizes[-1]; pressure_replies+=1
                elif strength>=5 and o.call<=8: action='call'; pressure_replies+=1
        else: action=rules[role].act(o)
        if session_data: session_data.action(g,o,seat,hero,action)
        if seat==hero and o.street==0 and isinstance(action,int): hero_raised=True
        if stats_book is not None:
            public='raise' if isinstance(action,int) else 'check' if action=='call' and not o.call else action
            stats_book.observe(f'seat-{seat}',o.street,public,o.call,o.pot)
        ref.apply(g,o,action,events)
    assert sum(g.payoffs)==0
    delta=[0]*6
    for i,seat in enumerate(g.order): delta[seat]=g.payoffs[i]
    assert all(s+p>=0 for s,p in zip(stacks,delta))
    pos=g.order.index(hero)
    refund=max(0,g.paid[pos]-max(x for i,x in enumerate(g.paid) if i!=pos))
    won=g.payoffs[pos]+g.paid[pos]>refund
    if session_data: session_data.finish(delta[hero]/2)
    return delta, won, hero_raised, adjusted, pressure_replies


def duplicate(job):
    seed,depth,field,common=job; stacks=stakes(depth,seed)
    candidate=control=wins=snaps=0
    for hero in range(6):
        roles=lineup(field,seed,hero)
        d,w,_,s,_=play(seed,hero,5,stacks,roles,True,common)
        c,*_=play(seed,hero,5,stacks,roles,False)
        candidate+=d[hero]/2;control+=c[hero]/2;wins+=w;snaps+=s
    if field=='reference':assert control==0
    return [100*candidate/6,100*control/6,100*(candidate-control)/6,100*wins/6,snaps]


def session(job):
    seed,depth,field,length=job; hero=(seed//1000)%6
    initial=stakes(depth,seed);roles=lineup(field,seed,hero); results=[]
    for candidate in (True,False):
        stacks=list(initial);net=peak=drawdown=busts=wins=opens=pressure=0
        for hand in range(length):
            delta,won,opened,_,replies=play(seed+hand,hero,(hero+hand)%6,stacks,roles,candidate,tendency=(hand,opens))
            wins+=won;opens+=opened;pressure+=replies
            stacks=[s+d for s,d in zip(stacks,delta)]
            net+=delta[hero]/2;peak=max(peak,net);drawdown=max(drawdown,peak-net)
            busts+=stacks[hero]==0
            # Unlimited 100-BB re-entries at zero, no top-ups after partial losses.
            stacks=[s or 200 for s in stacks]
        assert abs(net-(stacks[hero]-initial[hero]-200*busts)/2)<1e-9
        results.append([100*net/length,100*wins/length,drawdown,busts,pressure])
    c,b=results
    return [c[0],b[0],c[0]-b[0],c[1],c[2],b[2],c[3],b[3],c[4],b[4]]


def interval(values, z=1.96):
    mean=stats.mean(values); radius=z*stats.stdev(values)/math.sqrt(len(values))
    return [mean-radius,mean+radius]


def summary(rows):
    cols=list(zip(*rows));r={}
    for name,values in zip(('candidate_bb100','control_bb100','paired_delta_bb100','won_pct'),cols):
        r[name]=stats.mean(values);r[name+'_ci95']=interval(values)
    r['paired_delta_family_ci95']=interval(cols[2],stats.NormalDist().inv_cdf(1-.05/(2*FAMILY)))
    mid=len(rows)//2
    r['paired_delta_first_second_half']=[stats.mean(cols[2][:mid]),stats.mean(cols[2][mid:])]
    return r


def run(reference, blocks, sessions, length, workers, out, seed_offset):
    global alt_saved, empirical
    from poker.history_cfr import load
    for path,digest in FILES.items():
        with (reference/path).open('rb') as f: assert hashlib.file_digest(f,'sha256').hexdigest()==digest
    assert hashlib.sha256(BINARY.read_bytes()).hexdigest()==FROZEN
    empirical=json.loads((ROOT/'benchmarks/live-depths.json').read_text())['lineups_bb']
    ref.binary=str(BINARY); ref.saved=load(reference/BASE);alt_saved=load(reference/ALT)
    gc.collect();gc.freeze();out.mkdir(parents=True,exist_ok=True)
    results=[]
    with mp.get_context('fork').Pool(workers,initializer=init) as pool:
        for name,depth,field,common,start in CASES:
            start+=seed_offset
            rows=pool.map(duplicate,[(start+i,depth,field,common) for i in range(blocks)],chunksize=8)
            r=summary(rows);r.update(case=name,unit='deal block',units=blocks,candidate_hands=6*blocks,control_hands=6*blocks,seed=start,
                                      rounded_to_menu=sum(x[4] for x in rows),depth=depth)
            results.append(r); print(name,round(r['paired_delta_bb100'],2),flush=True)
        for name,depth,field,start in SESSION_CASES:
            start+=seed_offset
            rows=pool.map(session,[(start+i*1000,depth,field,length) for i in range(sessions)],chunksize=1)
            r=summary(rows);r.update(case=name,unit='session',units=sessions,hands_per_session=length,candidate_hands=sessions*length,control_hands=sessions*length,seed=start,depth=depth)
            for name2,ix,dd,bu,pr in [('candidate',0,4,6,8),('control',1,5,7,9)]:
                dds=[x[dd] for x in rows];busts=[x[bu] for x in rows]
                r[name2+'_session_risk']=dict(losing_pct=100*sum(x[ix]<0 for x in rows)/sessions,
                    p90_drawdown_bb=stats.quantiles(dds,n=10,method='inclusive')[8],max_drawdown_bb=max(dds),
                    busts=sum(busts),any_bust_pct=100*sum(b>0 for b in busts)/sessions,adaptive_replies=sum(x[pr] for x in rows))
            results.append(r);print(name,round(r['paired_delta_bb100'],2),flush=True)
    (out/'challenge.json').write_text(json.dumps(dict(binary_sha256=FROZEN,checkpoints=FILES,cases=results),indent=2)+'\n')


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('reference',type=Path)
    p.add_argument('--workers',type=int,default=6);p.add_argument('--blocks',type=int,default=6000)
    p.add_argument('--sessions',type=int,default=300);p.add_argument('--length',type=int,default=240);p.add_argument('--out',type=Path,default=OUT)
    p.add_argument('--seed-offset',type=int,default=17000000,help='production holdout offset; 0 for plumbing checks')
    a=p.parse_args();assert 4<=a.blocks<100000 and 4<=a.sessions<1000 and 1<=a.length<1000
    sys.path.insert(0,str(a.reference.resolve()));run(a.reference,a.blocks,a.sessions,a.length,a.workers,a.out,a.seed_offset)
