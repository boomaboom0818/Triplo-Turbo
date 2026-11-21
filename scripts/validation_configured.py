"""Supplementary paired session test at the incumbent's configured 50-BB buy-in."""
from collections import Counter
import gc
import hashlib
import json
import multiprocessing as mp
from pathlib import Path
import statistics
import sys

import benchmark_reference as ref
import validation_challenge as suite
from validation_breadth import OUT, BINARY, FROZEN


def test_session(ix):
    from poker.opponent_stats import OpponentStats
    seed=957000000+ix*1000;hero=ix%6;roles=suite.lineup('adaptive_mix',seed,hero)
    # Use only the current/fallback checkpoint, plus the scripted adaptation field.
    roles=['old' if x=='alt' else x for x in roles];rows=[]
    for challenger in (True,False):
        book=OpponentStats(min_faced=20);counts=Counter();stacks=[100]*6
        net=peak=dd=busts=opened=0
        for h in range(300):
            delta,_,op,_,_=suite.play(seed+h,hero,(hero+h)%6,stacks,roles,challenger,
                                     tendency=(h,opened),stats_book=book,counters=counts)
            opened+=op;stacks=[s+d for s,d in zip(stacks,delta)];busts+=stacks[hero]==0
            stacks=[s or 100 for s in stacks];net+=delta[hero]/2;peak=max(peak,net);dd=max(dd,peak-net)
        assert net==(stacks[hero]-100-100*busts)/2
        rows.append((net/3,dd,busts,counts))
    return rows


if __name__=='__main__':
    folder=Path(sys.argv[1]).resolve();sys.path.insert(0,str(folder));from poker.history_cfr import load
    assert hashlib.sha256(BINARY.read_bytes()).hexdigest()==FROZEN
    with (folder/suite.BASE).open('rb') as f:assert hashlib.file_digest(f,'sha256').hexdigest()==suite.FILES[suite.BASE]
    # All reference seats use the deployed model. No real player names or profiles.
    ref.binary=str(BINARY);ref.saved=load(folder/suite.BASE);suite.alt_saved=ref.saved;gc.collect();gc.freeze()
    sessions=int(sys.argv[2]) if len(sys.argv)>2 else 200;out=Path(sys.argv[3]) if len(sys.argv)>3 else OUT
    with mp.get_context('fork').Pool(6,initializer=suite.init) as pool:rows=pool.map(test_session,range(sessions),chunksize=1)
    differences=[a[0]-b[0] for a,b in rows];r=dict(binary_sha256=FROZEN,seed=957000000,sessions=sessions,hands_per_session=300,
        candidate_hands=sessions*300,control_hands=sessions*300,initial_and_rebuy_bb=50,paired_delta_bb100=statistics.mean(differences),
        paired_delta_ci95=suite.interval(differences),scope='Supplementary holdout; pointwise paired session CI. Incumbent wire adapter with public opponent stats learned within each branch.')
    for j,k in enumerate(('candidate','control')):
        group=[row[j] for row in rows];counts=sum((v[3] for v in group),Counter());r[k]=dict(bb100=statistics.mean(v[0] for v in group),
            p90_drawdown_bb=statistics.quantiles([v[1] for v in group],n=10,method='inclusive')[8],any_bust_pct=100*sum(v[2]>0 for v in group)/sessions,**counts)
    out.mkdir(parents=True,exist_ok=True);(out/'configured.json').write_text(json.dumps(r,indent=2)+'\n');print(json.dumps(r))
