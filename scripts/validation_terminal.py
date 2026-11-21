"""One-decision audit on recorded HU terminal call/fold offers, before Turbo went live.

No action values are fabricated for hands folded by the incumbent. Never expose
showdown cards to policy-stdio or persist private hands. NOT an offline win rate.
"""
import datetime as dt
import hashlib
import json
from collections import Counter
from pathlib import Path
import subprocess
import sys
from validation_breadth import BINARY, FROZEN, OUT

CUTOFF = dt.datetime(2026,11,21,10,4,28,tzinfo=dt.timezone.utc).timestamp()


def audit(root):
    assert hashlib.sha256(BINARY.read_bytes()).hexdigest()==FROZEN
    last=None;events={};spots={}; counts=Counter(); actions=Counter();costs=[]
    with (root/'trace.jsonl').open() as stream,subprocess.Popen([BINARY,'policy-stdio'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True) as bot:
        for line in stream:
            row=json.loads(line)
            if row['time']>CUTOFF:continue
            m=row['message'];h=m.get('hand_id');s=m.get('snapshot',{});h=s.get('hand_id',h)
            if not h:continue
            if h!=last:events={};last=h
            for e in (m,*m.get('replayed_events',[])):
                if e.get('type')=='player_action' and e.get('hand_id')==h and isinstance(e.get('table_seq'),int):
                    events[e['table_seq']]={k:e[k] for k in ('type','hand_id','seat','action','stack','contribution_delta') if k in e}
            hero=s.get('hero') or {};seat=hero.get('seat');menu=hero.get('valid_actions') or []
            if seat is None or seat!=s.get('actor_seat') or {a.get('action') for a in menu}!={'call','fold'}:continue
            bb=s.get('big_blind');paid=next(a.get('amount') for a in menu if a['action']=='call')
            eligible={p['seat']:p for p in s['seats'] if p.get('in_hand') and not p.get('folded')}
            if not bb or len(eligible)!=2 or seat not in eligible or not isinstance(paid,int) or paid<=0:continue
            other=next(p for i,p in eligible.items() if i!=seat);stack=eligible[seat]['stack'];cost=min(paid,stack)
            if not (other['stack']==0 or cost==stack or s.get('street')=='river'):continue
            counts['eligible_snapshots']+=1
            # Evaluate both assumptions about the single response budget; attribute
            # only actions they agree on. Earlier hands are the incumbent's history.
            rec=[]
            for used in (False,True):
                req=dict(profile=20,seed=s['table_seq'],response_used=used,events=[events[k] for k in sorted(events) if k<=s['table_seq']],
                         snapshot={k:s[k] for k in ('type','hand_id','big_blind','dealer_seat','board','pot','seats','hero')})
                # Remove identity/presentation and action-authority metadata before IPC.
                req['snapshot']['hero']={k:hero[k] for k in ('seat','hole_cards','valid_actions')}
                req['snapshot']['seats']=[{k:p[k] for k in ('seat','in_hand','folded','stack','bet') if k in p} for p in s['seats']]
                bot.stdin.write(json.dumps(req)+'\n');bot.stdin.flush();a=json.loads(bot.stdout.readline())['action'];assert a in ('fold','call');rec.append(a)
            spots[h]=(seat,bb,stack,cost,s['pot'],rec)
        bot.stdin.close();assert bot.wait()==0
    # Join only after recommendations are frozen. No private board runouts needed.
    counts['unique_eligible_hands']=len(spots);unknown_lo=unknown_hi=known=0.;done=set()
    with (root/'session.jsonl').open() as stream:
        for line in stream:
            row=json.loads(line)
            if row['time']>CUTOFF or row.get('type')!='hand_result':continue
            res=row['result'];h=res.get('hand_id');trail=res.get('actions') or []
            if h not in spots or h in done or not trail:continue
            seat,bb,stack,cost,pot,rec=spots[h];last=trail[-1]
            if last.get('seat')!=seat or last.get('action') not in ('fold','call'):continue
            end=res.get('final_stacks',{}).get(str(seat));old=last['action']
            if not isinstance(end,int) or (old=='fold' and end!=stack):continue
            done.add(h);counts['confirmed_terminal_results']+=1
            if rec[0]!=rec[1]:counts['budget_ambiguous']+=1;continue
            new=rec[0];actions[f'{old}_to_{new}']+=1
            if old==new:continue
            if old=='call':
                gain=(stack-end)/bb; known+=gain;costs.append(gain)
            else:
                unknown_lo-=cost/bb;unknown_hi+=pot/bb
    r=dict(binary_sha256=FROZEN,cutoff_utc='2026-11-21T10:04:28Z',counts=dict(counts),unconfirmed=len(spots)-len(done),
           incumbent_to_candidate=dict(actions),known_fold_instead_of_call_net_bb=known,known_fold_change_min_max_bb=[min(costs),max(costs)] if costs else None,
           unknown_call_instead_of_fold_bound_bb=[unknown_lo,unknown_hi],identified_and_bounded_total_delta_bb=[known+unknown_lo,known+unknown_hi],
           interpretation='Conditional on incumbent-reached, confirmed, two-player terminal spots with agreement across both response budgets. Unknown call returns bounded by losing call cost or gaining the entire pot before the call. Not an estimate of whole-policy EV.')
    (OUT/'terminal.json').write_text(json.dumps(r,indent=2)+'\n');print(json.dumps(r))


if __name__=='__main__':audit(Path(sys.argv[1]))
