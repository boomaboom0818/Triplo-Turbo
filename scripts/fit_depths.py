"""Compress six-handed preflop snapshots into five anonymous stack lineups.

One observation per hand, no weighting by action count. Export group medians
only: no hand IDs, cards, timestamps, exact stacks, or player identifiers.
"""
import collections
import json
import statistics
import sys

seen=set();lineups=[];effective=[];occupancy=collections.Counter()
with open(sys.argv[1]) as stream:
    for line in stream:
        row=json.loads(line);m=row.get('message',row);s=m.get('snapshot',{});hid=s.get('hand_id')
        if not hid or hid in seen or s.get('street')!='preflop':continue
        seats=[x for x in s['seats'] if x.get('in_hand')];h=s.get('hero',{}).get('seat');bb=s.get('big_blind',20)
        stacks={x['seat']:(x['bet']+x['stack'])/bb for x in seats}
        if h not in stacks or len(stacks)<2:continue
        seen.add(hid);occupancy[len(stacks)]+=1
        effective.append(min(stacks[h],max(v for i,v in stacks.items() if i!=h)))
        if len(seats)==6:lineups.append(sorted(stacks.values()))
lineups.sort(key=statistics.median);groups=[]
for i in range(5):
    group=lineups[len(lineups)*i//5:len(lineups)*(i+1)//5]
    groups.append([max(5,5*round(statistics.median(col)/5)) for col in zip(*group)])
print(json.dumps(dict(hands=len(seen),occupancy=occupancy,six_max_hands=len(lineups),
                      effective_quartiles_bb=statistics.quantiles(effective,n=4),
                      effective_over_250bb=sum(x>250 for x in effective),lineups_bb=groups),indent=2))
