"""Black-box river test against an independently enumerated heads-up oracle."""
import argparse
from collections import Counter
from itertools import combinations
import hashlib
import json
import math
from pathlib import Path
import random
import statistics as stats
import subprocess
import sys

from validation_breadth import BINARY, FROZEN, OUT


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('reference',type=Path)
    a=p.parse_args();sys.path.insert(0,str(a.reference.resolve()))
    from poker.fast_eval import evaluate, NAMES, CARD_ID
    assert hashlib.sha256(BINARY.read_bytes()).hexdigest()==FROZEN
    rng=random.Random(92900123);counts=Counter();bad=Counter();examples=[];errors=[]
    def fail(kind,board,hole,out):
        bad[kind]+=1
        if len(examples)<10:examples.append(dict(check=kind,board=[NAMES[c] for c in board],hole=[NAMES[c] for c in hole],output=out))
    with subprocess.Popen([BINARY,'policy-stdio'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1) as bot:
        def ask(board,hole,paid,used,seed):
            # Heads up against either a check or the last player's all-in.
            bb=20;cost=2000 if paid else 0
            menu=[{'action':'fold'},{'action':'call','amount':cost}] if paid else [
                {'action':'fold'},{'action':'check'},{'action':'raise','min':20,'max':2000},{'action':'all_in','amount':2000}]
            snap=dict(type='table_state',hand_id='test',board=[NAMES[c] for c in board],pot=100+cost,big_blind=bb,dealer_seat=0,
                      seats=[dict(seat=0,stack=2000,bet=0,in_hand=True,folded=False),dict(seat=1,stack=2000-cost,bet=cost,in_hand=True,folded=False)],
                      hero=dict(seat=0,hole_cards=[NAMES[c] for c in hole],valid_actions=menu))
            req=dict(seed=seed,profile=20,response_used=used,events=[],snapshot=snap)
            bot.stdin.write(json.dumps(req)+'\n');bot.stdin.flush();out=json.loads(bot.stdout.readline());counts['decisions']+=1
            act=out['action'];names={m['action'] for m in menu}
            if isinstance(act,dict):allowed='raise' in names and 20<=act.get('raise',-1)<=2000
            else:allowed=act in names
            if not allowed:fail('illegal_action',board,hole,out)
            if not paid and act=='fold':fail('fold_when_check_free',board,hole,out)
            return out

        boards=[tuple(rng.sample(range(52),5)) for _ in range(3000)]
        # Structured play-the-board and tie-heavy examples in addition to random boards.
        boards += [tuple(CARD_ID[c] for c in b.split()) for b in (
            'Ah Kh Qh Jh Th','2c 2d 2h 2s Ac','Tc Jd Qs Kh Ac','As Ad Ah Kc Kd',
            '6h 7h 8h 9h Th','2s 3c 4h 5d 6h','Ks Kh Kd Kc Qs')]
        for i,b in enumerate(boards):
            remaining=sorted(set(range(52))-set(b));pairs=list(combinations(remaining,2))
            scores={h:evaluate(b+h) for h in pairs};best=max(scores.values());top=rng.choice([h for h,s in scores.items() if s==best]);random_h=rng.choice(pairs)
            for used in (False,True):
                ans=ask(b,top,True,used,i);counts['nut_call_tests']+=1
                if ans['action']!='call':fail('folded_unbeatable_hand',b,top,ans)
            ans=ask(b,random_h,False,False,i);hs=scores[random_h]
            # Remove hero blockers; exactly C(45,2)=990 opponent combos remain.
            other=[s for (x,y),s in scores.items() if x not in random_h and y not in random_h];assert len(other)==990
            exact=sum((s<hs)+.5*(s==hs) for s in other)/990
            if i<3000:errors.append(ans['equity']-exact)
            if i%25==0:
                counts['repeat_tests']+=1
                if ask(b,random_h,False,False,i)!=ans:fail('non_reproducible',b,random_h,ans)
        bot.stdin.close();assert bot.wait()==0
    result=dict(binary_sha256=FROZEN,seed=92900123,boards=len(boards),counts=dict(counts),violations=dict(bad),examples=examples,
                equity_uniform_no_wager=dict(random_boards=len(errors),samples_per_estimate=512,bias=stats.mean(errors),
                    rmse=math.sqrt(stats.mean(x*x for x in errors)),p99_absolute_error=stats.quantiles(list(map(abs,errors)),n=100,method='inclusive')[98]))
    OUT.mkdir(parents=True,exist_ok=True);(OUT/'river.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result))
    if bad:raise SystemExit('Dominated, illegal, or non-reproducible action detected; see river.json')


if __name__=='__main__':main()
