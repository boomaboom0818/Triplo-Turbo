"""Differentially test the payoff engine used for the hard benchmarks against PokerKit.
This audits simulation mechanics, not candidate strength. No model or live I/O.
"""
from collections import Counter
import json
from pathlib import Path
import random
import sys
from validation_breadth import OUT


def run(reference):
    sys.path.insert(0,str(reference));from poker.fast_game import FastGame
    from poker.game import Game
    import importlib.metadata
    seen=Counter();failures=Counter();examples=[];start=944170000
    for seed in range(start,start+12000):
        rng=random.Random(seed);n=rng.randint(3,6);button=rng.randrange(n)
        stacks=[rng.choice([1,2,3,5,11,25,41,77,200,800,rng.randint(2,3000)]) for _ in range(n)]
        slow=Game(seed,n,button,stacks);fast=FastGame(seed,n,button,stacks)
        trail=[];problem=None
        while slow.state.status and fast.live:
            o=slow.observe();f=fast.observe();seen['decision_points']+=1
            for field in ('player','position','stacks','pot','call','board','hole','active','street'):
                if getattr(o,field)!=getattr(f,field):problem='observed_'+field;break
            if problem:break
            if o.legal!=f.legal:problem='legal_menu';break
            sizes=[a for a in o.legal if isinstance(a,int)]
            choices=['call']*5+(['fold'] if 'fold' in o.legal else [])
            if sizes:
                lo,hi=min(sizes),max(sizes)
                choices.extend([lo,hi,min(lo+1,hi),max(hi-1,lo),rng.randint(lo,hi)])
            action=rng.choice(choices);trail.append(action)
            if isinstance(action,int):
                assert slow.state.can_complete_bet_or_raise_to(action)
                slow.state.complete_bet_or_raise_to(action)
                seen['off_menu_raises']+=action not in o.legal
                seen['stack_committing_raises']+=action==hi
            elif action=='call':slow.state.check_or_call()
            else:slow.state.fold()
            slow.advance();fast.act(action,checked=False)
        if not problem and slow.state.status!=fast.live:problem='terminal_state'
        if not problem and slow.payoff()!=fast.payoff():problem='settled_payouts'
        if problem:
            failures[problem]+=1
            if len(examples)<12:examples.append(dict(seed=seed,seats=n,button=button,stacks=stacks,actions=trail,issue=problem))
        else:seen['settlements_matched']+=1
    r=dict(hands=12000,seed=start,pokerkit=importlib.metadata.version('pokerkit'),counters=dict(seen),discrepancies=dict(failures),examples=examples)
    OUT.mkdir(parents=True,exist_ok=True);(OUT/'rules.json').write_text(json.dumps(r,indent=2)+'\n');print(json.dumps(r))


if __name__=='__main__':run(Path(sys.argv[1]).resolve())
