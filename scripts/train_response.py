"""Paired full-hand action-value training against the frozen independent checkpoint.

Generate independent hidden deals offline. At qualifying observed decisions, fork
the deal, change exactly one action, and play both branches to completion. A
runtime policy only gets the bucket computed from its own cards and public state.
"""
import argparse
import collections
import gc
import json
import math
import multiprocessing as mp
import random
import sys
from pathlib import Path
import benchmark_reference as ref


def shard(job):
    from poker.fast_game import FastGame
    start,count=job
    acc=collections.defaultdict(lambda:[0,0.,0.])
    for seed in range(start,start+count):
        hero=seed%6;depth=(50,100,100,200)[seed//6%4];g=FastGame(seed,6,button=5,stacks=[depth*2]*6,lean=True)
        for i,p in enumerate(ref.field):p.rng=random.Random(f'train:{seed}:{i}')
        rng=random.Random(seed);events=[];pending=[];streets=set()
        while g.live:
            o=g.observe();i=o.position
            if i==hero:
                action,probe=ref.hero_decision(g,o,17,events,rng,probe=o.street not in streets)
                if probe:
                    bucket,choices=probe;streets.add(o.street)
                    states=[p.rng.getstate() for p in ref.field];hstate=rng.getstate();cache={}
                    for slot,alt in enumerate(choices):
                        if alt is None or alt==action:continue
                        if alt not in cache:
                            branch=g.clone();hist=list(events);ref.apply(branch,o,alt,hist)
                            for p,st in zip(ref.field,states):p.rng.setstate(st)
                            rng.setstate(hstate)
                            while branch.live:
                                obs=branch.observe();actor=obs.position
                                nxt=ref.hero_decision(branch,obs,17,hist,rng)[0] if actor==hero else ref.field[actor].act(obs,history_complete=True)
                                ref.apply(branch,obs,nxt,hist)
                            cache[alt]=branch.payoffs[hero]/2
                        pending.append((bucket,slot,cache[alt]))
                    for p,st in zip(ref.field,states):p.rng.setstate(st)
                    rng.setstate(hstate)
            else:action=ref.field[i].act(o,history_complete=True)
            ref.apply(g,o,action,events)
        # One observation per bucket/slot per hand, even if two streets share a bucket.
        per_hand=collections.defaultdict(list)
        for key,slot,net in pending:per_hand[key,slot].append(net-g.payoffs[hero]/2)
        for k,values in per_hand.items():
            d=sum(values)/len(values);row=acc[k];row[0]+=1;row[1]+=d;row[2]+=d*d
    return dict(acc)


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('reference',type=Path);p.add_argument('--hands',type=int,default=35000)
    p.add_argument('--workers',type=int,default=6);p.add_argument('--seed',type=int,default=73900001);a=p.parse_args();assert a.hands>=1000
    sys.path.insert(0,str(a.reference.resolve()));from poker.history_cfr import load
    ref.saved=load(a.reference/'runs/bot/pressure-fresh-001/base-guard.json');ref.binary=str(Path(__file__).resolve().parents[1]/'target/release/triplo-turbo');gc.collect();gc.freeze()
    acc=collections.defaultdict(lambda:[0,0.,0.]);jobs=[(a.seed+i,min(60,a.hands-i)) for i in range(0,a.hands,60)]
    with mp.get_context('fork').Pool(a.workers,initializer=ref.init) as pool:
        for rows in pool.imap(shard,jobs):
            for k,v in rows.items():
                for i,d in enumerate(v):acc[k][i]+=d
    actions={};bounds={};trials=0
    for (k,slot),(n,s,ss) in sorted(acc.items()):
        trials+=n
        if n<75:continue
        lower=s/n-2.58*math.sqrt(max(0.,ss-s*s/n)/(n-1)/n)
        if lower>max(.04,bounds.get(k,0)):
            actions[k]=slot;bounds[k]=lower
    print(json.dumps(dict(schema=1,base_profile=17,seed=a.seed,hands=a.hands,paired_comparisons=trials,actions=actions),indent=2))
