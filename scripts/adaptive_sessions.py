"""Three policies, matched deals and independent session trajectories. Offline only."""
import argparse
import atexit
from collections import Counter
import gc
import gzip
import hashlib
from itertools import count
import json
import multiprocessing as mp
from pathlib import Path
import statistics as st
import subprocess
import sys
import benchmark_reference as ref
import validation_challenge as suite
from validation_breadth import BINARY as OLD, ROOT, FROZEN

NEW = ROOT / 'target/release/triplo-turbo'
OUT = ROOT / 'benchmarks/adaptation'
FIELDS = ['reactive100','configured50','mixed','static100','elastic100']


class Record:
    ids=count()
    def __init__(self, branch):
        # Opaque stream/hand labels for the reducer; deal seeds stay in the engine.
        self.branch, self.key = branch, str(next(self.ids))
        self.hands=0
        self.count = Counter(); self.pending=[]

    def start(self, hero):
        self.hands+=1; self.hand=str(self.hands); self.hero=hero; self.bucket=None; self.reply='foldthrough'; self.log=[]

    def request(self, g, i, snap):
        if self.branch!=21: return {}
        snap['hand_id']=self.hand
        for p,seat in zip(snap['seats'],g.order): p['name']=str(seat)
        out=dict(history_session=self.key, completed_hands=self.pending)
        self.pending=[]
        return out

    def action(self, g,o,seat,hero,a):
        if o.street: return
        raise_=isinstance(a,int)
        if self.branch==21:
            public='raise' if raise_ else 'check' if a=='call' and not o.call else a
            self.log.append(dict(type='player_action',hand_id=self.hand,seat=seat,name=str(seat),street='preflop',action=public,to_call_before=o.call*10))
        if seat==hero and self.branch and self.reason.startswith('defended_'): self.count[self.reason]+=1
        if seat==hero and raise_ and self.bucket is None:
            from poker.rule_opponents import chen
            strength=chen(o.hole)  # reporting only; never sent to Turbo
            self.bucket=('weak' if strength<7 else 'middle' if strength<10 else 'strong')+('_reraise' if o.call>2 else '_open')
        elif seat!=hero and self.bucket and self.reply!='reraise':
            if raise_: self.reply='reraise'
            elif o.call and a=='call': self.reply='call'

    def finish(self, bb):
        if self.branch==21:
            self.pending.append([dict(type='hand_start',hand_id=self.hand,seat=self.hero),*self.log])
        if self.bucket:
            for key in [self.bucket,'all']:
                self.count[key+':'+self.reply]+=1;self.count[key+':net_bb']+=bb


def init():
    global new_bot,old_bot
    suite.init();old_bot=ref.bot
    if phase!='diagnose':
        new_bot=subprocess.Popen([NEW,'policy-stdio'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
        atexit.register(new_bot.terminate)


def session(job):
    from poker.opponent_stats import OpponentStats
    idx, field, start=job; seed=start+idx*1000;hero=idx%6;cfg=field=='configured50'
    if not cfg:
        for policy in ref.field:
            policy.fallback.profiles={}
    roles=suite.lineup('archive_mix' if field=='mixed' else 'adaptive_mix',seed,hero)
    if cfg: roles=['old' if r=='alt' else r for r in roles]
    if field in ('elastic100','static100'):
        roles=[('elastic' if field=='elastic100' else 'aggressive-v1') if r=='adaptive' else r for r in roles]
    length=300 if cfg else 240; rebuy=100 if cfg else 200
    initial=suite.stakes('empirical' if field=='mixed' else rebuy//2,seed)
    out={}
    for branch in ([20] if phase=='diagnose' else [21,20,0]):
        if branch:ref.bot=old_bot if branch==20 else new_bot
        book=OpponentStats(min_faced=20) if cfg else None; record=Record(branch)
        stacks=initial.copy();net=peak=dd=busts=won=opens=0
        for h in range(length):
            delta,w,op,_,_=suite.play(seed+h,hero,(hero+h)%6,stacks,roles,bool(branch),tendency=(h,opens),
                stats_book=book,counters=Counter(),profile=branch,session_data=record)
            opens+=op;won+=w;stacks=[s+d for s,d in zip(stacks,delta)];busts+=stacks[hero]==0
            stacks=[s or rebuy for s in stacks];net+=delta[hero]/2;peak=max(peak,net);dd=max(dd,peak-net)
        assert net==(stacks[hero]-initial[hero]-rebuy*busts)/2
        out[branch]=dict(bb100=100*net/length,dd=dd,busts=busts,won=won,opens=opens,events=dict(record.count))
    return out


def digest(path):
    with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('reference',type=Path);p.add_argument('phase',choices=['diagnose','dev','confirm']);p.add_argument('--sessions',type=int,required=True);p.add_argument('--workers',type=int,default=8);p.add_argument('--fields',nargs='+',choices=FIELDS,default=FIELDS);p.add_argument('--output',type=Path)
    args=p.parse_args();phase=args.phase; assert 2<=args.sessions<=900
    folder=args.reference.resolve(); sys.path.insert(0,str(folder));from poker.history_cfr import load
    assert digest(OLD)==FROZEN
    for k,v in suite.FILES.items():assert digest(folder/k)==v
    ref.binary=str(OLD);ref.saved=load(folder/suite.BASE);suite.alt_saved=load(folder/suite.ALT)
    suite.empirical=json.loads((ROOT/'benchmarks/live-depths.json').read_text())['lineups_bb'];gc.collect();gc.freeze()
    base={'diagnose':1100000000,'dev':1200000000,'confirm':1300000000}[phase]
    report=dict(phase=phase,frozen=FROZEN,candidate=digest(NEW) if phase!='diagnose' else None,
                checkpoints=suite.FILES,fields={});observations=[]
    with mp.get_context('fork').Pool(args.workers,initializer=init) as pool:
        for field in args.fields:
            start=base+FIELDS.index(field)*1000000
            rows=pool.map(session,[(i,field,start) for i in range(args.sessions)],chunksize=1)
            length=300 if field=='configured50' else 240
            v=dict(seed=start,sessions=len(rows),hands_per_branch=len(rows)*length,branches={},paired={})
            for b in rows[0]:
                r=[x[b] for x in rows];events=Counter()
                for x in r: events.update(x['events'])
                v['branches'][b]=dict(bb100=st.mean(x['bb100'] for x in r),p90_dd=st.quantiles([x['dd'] for x in r],n=10,method='inclusive')[8],
                    bust_session_pct=100*st.mean(x['busts']>0 for x in r),win_pct=100*sum(x['won'] for x in r)/v['hands_per_branch'],events=dict(events))
            if 21 in rows[0]:
                for b in (20,0):
                    ds=[x[21]['bb100']-x[b]['bb100'] for x in rows];v['paired'][b]=dict(delta=st.mean(ds),ci95=suite.interval(ds),
                        family95=suite.interval(ds,st.NormalDist().inv_cdf(1-.05/(4*len(args.fields)))))
            if phase=='confirm':
                observations.extend(dict(field=field,seed=start+i*1000,branches={b:{k:n for k,n in x.items() if k!='events'} for b,x in trial.items()}) for i,trial in enumerate(rows))
            report['fields'][field]=v;print(field,{b:round(r['bb100'],1) for b,r in v['branches'].items()},flush=True)
    OUT.mkdir(parents=True,exist_ok=True)
    if phase=='confirm':
        assert report['candidate']==digest(NEW)
        with gzip.open(OUT/'sessions.jsonl.gz','wt') as stream:
            for row in observations:stream.write(json.dumps(row,separators=(',',':'))+'\n')
    (args.output or OUT/(phase+'.json')).write_text(json.dumps(report,indent=2)+'\n')
