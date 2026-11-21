"""Fit open choices from terminal payoffs, replaying each supported alternative
on the identical deal. Only the unmodified policy advances the training session.
The Rust probe defines the features, admissible sizes and eligibility at runtime.
"""
import argparse
from collections import defaultdict
import gc
import json
import math
import multiprocessing as mp
import statistics
import sys
from adaptive_sessions import Record, NEW, ROOT, OUT, digest
import validation_challenge as suite
import benchmark_reference as ref


class Teacher(Record):
    def start(self,hero):
        super().start(hero);self.step=0;self.spot=None
    def request(self,g,i,snap):
        return dict(super().request(g,i,snap),steal_probe=True)
    def intervene(self,reply):
        self.step+=1
        if reply.get('steal_options') and self.spot is None:
            self.spot=(self.step, reply['action'], *reply['steal_options'])


class Fork(Record):
    def __init__(self,step,action):
        super().__init__(20);self.remaining=step;self.chosen=action;self.hit=False
    def intervene(self,reply):
        self.remaining-=1
        if self.remaining==0:
            reply.update(action=self.chosen,spends_response=True,reason='forced_alternative');self.hit=True


def trajectories(job):
    ix,field=job; seed=1400000000+1000000*field+1000*ix;hero=ix%6
    depth=(100,100,'empirical',50)[field];roles=suite.lineup('archive_mix' if field==2 else 'adaptive_mix',seed,hero)
    if field==1:roles=['aggressive-v1' if r=='adaptive' else r for r in roles]
    stacks=suite.stakes(depth,seed);rebuy=100 if depth==50 else 200
    obs=Teacher(21);opens=0;stats=defaultdict(lambda:[0,0.0]);forks=0
    for h in range(240):
        args=(seed+h,hero,(hero+h)%6,stacks,roles,True)
        delta,_,opened,*_=suite.play(*args,tendency=(h,opens),profile=21,session_data=obs)
        if obs.spot:
            step,base,key,opts=obs.spot;outcomes={json.dumps(base):delta[hero]}
            for i,opt in enumerate(opts):
                if opt is None:continue
                k=json.dumps(opt)
                if k not in outcomes:
                    alt=Fork(step,opt);d,*_=suite.play(*args,tendency=(h,opens),profile=20,session_data=alt)
                    assert alt.hit;outcomes[k]=d[hero];forks+=1
                row=stats[key,i];row[0]+=1;row[1]+=(outcomes[k]-delta[hero])/2
        opens+=opened;stacks=[s+d or rebuy for s,d in zip(stacks,delta)]
    return dict(stats),forks


if __name__=='__main__':
    from pathlib import Path
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('reference',type=Path);p.add_argument('--sessions',type=int,default=125);p.add_argument('--workers',type=int,default=8);a=p.parse_args();folder=a.reference.resolve();sys.path.insert(0,str(folder));from poker.history_cfr import load
    for k,v in suite.FILES.items():assert digest(folder/k)==v
    ref.binary=str(NEW);ref.saved=load(folder/suite.BASE);suite.alt_saved=load(folder/suite.ALT);suite.empirical=json.loads((ROOT/'benchmarks/live-depths.json').read_text())['lineups_bb'];gc.collect();gc.freeze()
    with mp.get_context('fork').Pool(a.workers,initializer=suite.init) as pool:
        groups=pool.map(trajectories,[(j,i) for i in range(4) for j in range(a.sessions)],chunksize=1)
    cells={};chosen={}
    for key in range(12):
        candidates=[]
        for i in range(4):
            vals=[row.get((key,i),(0,0)) for row,_ in groups];n=sum(x for x,_ in vals)
            if n<250:continue
            mean=sum(y for _,y in vals)/n
            se=statistics.stdev(y-mean*x for x,y in vals)*math.sqrt(len(vals))/n
            byfield=[vals[j:j+a.sessions] for j in range(0,len(vals),a.sessions)]
            means=[sum(y for _,y in group)/max(1,sum(x for x,_ in group)) for group in byfield];lower=mean-2*se
            cells[f'{key}:{i}']=dict(n=n,mean=mean,se=se,fields=means,lower=lower)
            if lower>0.04 and min(means)>-0.25:candidates.append((lower,-i))
        if candidates:chosen[key]=-max(candidates)[1]
    OUT.mkdir(parents=True,exist_ok=True)
    (OUT/'fit.json').write_text(json.dumps(dict(seed=1400000000,binary=digest(NEW),sessions=len(groups),hands=len(groups)*240,forks=sum(x for _,x in groups),cells=cells,choices=chosen),indent=2)+'\n')
    (ROOT/'models/adaptive.json').write_text(json.dumps(dict(base=20,choices=chosen),indent=2)+'\n');print('Selected',chosen,flush=True)
