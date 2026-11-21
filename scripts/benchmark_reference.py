"""Offline independent six-max test against the existing Triplo research checkout.

Use that checkout's Python venv. Only hero's legal, player-visible snapshot is
sent to Rust. No network, live traces, real keys, or checkpoint modification.
"""
import argparse
import atexit
import gc
import json
import math
import multiprocessing as mp
import random
import statistics
import subprocess
import sys
from pathlib import Path


def init():
    global bot, field
    from poker.history_cfr import HistoryPolicy
    from poker.equity_pricing import make_fallback
    bot = subprocess.Popen([binary, 'policy-stdio'], stdin=subprocess.PIPE,
                           stdout=subprocess.PIPE, text=True, bufsize=1)
    atexit.register(bot.terminate)
    field = [HistoryPolicy(saved, min_visits=64) for _ in range(6)]
    for policy in field:
        policy.fallback = make_fallback(globals().get('fallback_name','equity-guarded-v3'))


def hero_decision(g, o, profile, events, rng, probe=False, session=None):
    i=o.position
    legal = [dict(action='fold')] if 'fold' in o.legal else []
    legal.append(dict(action='call', amount=o.call*10) if o.call else dict(action='check'))
    if g.can_raise():
        legal.append(dict(action='raise',min=10*min(g.stacks[i]+g.bets[i],max(2,g.raise_size)+max(g.bets)),max=10*(g.stacks[i]+g.bets[i])))
    snap=dict(type='table_state',hand_id='h',dealer_seat=5,board=o.board,big_blind=20,pot=10*o.pot,
              hero=dict(seat=i,hole_cards=o.hole,valid_actions=legal),seats=[dict(seat=j,stack=s*10,bet=g.bets[j]*10,in_hand=True,folded=not o.active[j]) for j,s in enumerate(o.stacks)])
    extra={}
    if session:
        extra=session.request(g, i, snap)
    wire_events=events if snap['hand_id']=='h' else [dict(e,hand_id=snap['hand_id']) for e in events]
    bot.stdin.write(json.dumps(dict(profile=profile,probe=probe,response_used=any(e.get('response_used') for e in events),snapshot=snap,events=wire_events,seed=rng.getrandbits(64),**extra))+'\n');bot.stdin.flush()
    r=json.loads(bot.stdout.readline())
    if session and hasattr(session,'intervene'): session.intervene(r)
    if session: session.reason=r['reason']
    if profile>=19 and r.get('spends_response',r['reason']=='trained_response'):events.append({'type':'offline_policy_marker','response_used':True})
    def convert(a):
        if isinstance(a,dict):return max(legal[-1]['min']//10,min(legal[-1]['max']//10,round(a['raise']/10)))
        return 'call' if a in ('check','call') else a
    alt=r.get('probe');return convert(r['action']), (alt[0],[convert(a) for a in alt[1]]) if alt else None


def apply(g,o,act,events):
    i=o.position
    st_names=('preflop','flop','turn','river')
    pot_b=10*o.pot
    owing=10*o.call
    if isinstance(act,int):
        kind='raise'
        delta=max(0,10*act-10*g.bets[i])
    elif act=='call' and not o.call:
        kind='check'
        delta=0
    elif act=='call':
        kind='call'
        delta=min(owing,10*g.stacks[i])
    else:
        kind=act
        delta=0
    if isinstance(act,int) and act not in o.legal:
        assert g.can_raise() and max(g.bets)<act<=g.bets[i]+g.stacks[i]
        g.history.append((o.player,i,o.street,act,o.pot,o.call));g.act(act,checked=False)
    else:g.act(act)
    next_st='showdown' if not g.live else (st_names[g.street] if g.street<len(st_names) else 'river')
    events.append(dict(type='player_action',hand_id='h',seat=i,street=next_st,action=kind,pot_before=pot_b,to_call_before=owing,contribution_delta=delta))


def block(job):
    seed,profile,depth,lineup=job
    from poker.fast_game import FastGame
    from poker.rule_opponents import RulePolicy
    styles=[RulePolicy('station-v1'),RulePolicy('aggressive-v1')];wins = net = control = 0
    if depth=='empirical': stacks=list(empirical_lineups[seed%len(empirical_lineups)])
    else: stacks=[35,60,100,150,250,500] if depth=='mixed' else [int(depth)]*6
    random.Random(seed).shuffle(stacks)
    for hero in range(6):
        for challenger in ([True,False] if lineup=='mixed' else [True]):
            g = FastGame(seed, 6, button=5, stacks=[2*s for s in stacks], lean=True)
            for i, p in enumerate(field):p.rng = random.Random(f'{seed}:{i}')
            rng = random.Random(seed * 6 + hero);events=[]
            while g.live:
                o = g.observe();i = o.position;rel=(i-hero)%6
                if challenger and i==hero:act,_=hero_decision(g,o,profile,events,rng)
                elif lineup=='mixed' and rel in (2,4):act=styles[rel//2-1].act(o)
                else:act=field[i].act(o,history_complete=True)
                apply(g,o,act,events)
            if challenger:
                returned=max(0,g.paid[hero]-max(x for j,x in enumerate(g.paid) if j!=hero))
                wins+=g.payoffs[hero]+g.paid[hero]>returned;net+=g.payoffs[hero]/2
            else:control+=g.payoffs[hero]/2
    # Identical checkpoint copies with identical per-seat RNGs have exactly zero
    # total return in a six-seat rotation on the same deal, by chip conservation.
    return 100*wins/6,100*net/6,100*control/6,100*(net-control)/6


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('reference',type=Path);p.add_argument('--blocks',type=int,default=400)
    p.add_argument('--seed',type=int,default=8901001);p.add_argument('--workers',type=int,default=4)
    p.add_argument('--profiles',type=int,nargs='+',default=[20]);p.add_argument('--depths',nargs='+',default=['100']);p.add_argument('--field',choices=['checkpoint','mixed'],default='checkpoint')
    p.add_argument('--checkpoint',default='runs/bot/pressure-fresh-001/base-guard.json')
    p.add_argument('--fallback',choices=['equity-guarded-v3','equity-v1'],default='equity-guarded-v3')
    args=p.parse_args();assert args.blocks>=2
    empirical_lineups=json.loads((Path(__file__).resolve().parents[1]/'benchmarks/live-depths.json').read_text())['lineups_bb'] if 'empirical' in args.depths else []
    binary=str(Path(__file__).resolve().parents[1]/'target/release/triplo-turbo')
    sys.path.insert(0,str(args.reference.resolve()))
    from poker.history_cfr import load
    fallback_name=args.fallback;saved=load(args.reference/args.checkpoint);gc.collect();gc.freeze()
    reports=[]
    with mp.get_context('fork').Pool(args.workers,initializer=init) as pool:
        for depth in args.depths:
            for profile in args.profiles:
                rows=pool.map(block, [(seed,profile,depth,args.field) for seed in range(args.seed,args.seed+args.blocks)],chunksize=4)
                report=dict(checkpoint=args.checkpoint,fallback=args.fallback,opponents='5 x trained reference (no persistent stats)' if args.field=='checkpoint' else '3 x trained reference, station, aggressive; paired champion control',
                            profile=profile,hands=6*args.blocks,deal_blocks=args.blocks,seed=args.seed,stack_bb=depth)
                for name,values in zip(('won_pct','bb_per_100','control_bb_per_100','paired_delta_bb_per_100'),zip(*rows)):
                    mean=statistics.mean(values);delta=1.96*statistics.stdev(values)/math.sqrt(len(values))
                    report[name]=mean;report[name+'_block_95pct_ci']=[mean-delta,mean+delta]
                print(f"profile {profile}, {depth} BB: {report['bb_per_100']:.2f} bb/100",file=sys.stderr,flush=True)
                reports.append(report)
    print(json.dumps(reports,indent=2))
