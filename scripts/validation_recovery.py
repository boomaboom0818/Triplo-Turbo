"""Replay the same immutable prefix of the incumbent's own redacted capture.

Only aggregate counts and the input fingerprint are retained. This exercises
the offline reducer; transport retries are covered separately by WebSocket tests.
"""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from validation_breadth import BINARY, FROZEN, OUT


def run(path, size, mode):
    inserted=removed=sent=0;last=None;digest=hashlib.sha256();used=0
    with path.open('rb') as capture, subprocess.Popen([BINARY,'replay','/dev/stdin'],stdin=subprocess.PIPE,stdout=subprocess.PIPE) as proc:
        while used<size:
            raw=capture.readline(size-used);used+=len(raw)
            if not raw or not raw.endswith(b'\n'):break
            digest.update(raw)
            row=json.loads(raw);m=row.get('message',row)
            if not isinstance(m,dict):continue
            seq=m.get('table_seq');kind=m.get('type');table=m.get('table_id')
            if mode=='sparse_history' and kind=='player_action' and isinstance(seq,int) and seq%4!=0:
                removed+=1;continue
            if mode!='clean' and kind=='resync_response':
                m['replayed_events']=list(reversed(m.get('replayed_events',[])))
                raw=json.dumps(row).encode()+b'\n'
            proc.stdin.write(raw);sent+=1
            if mode!='clean' and isinstance(seq,int):
                if seq%3==0:
                    proc.stdin.write(raw);inserted+=1
                if last and table and last[0]==table and last[1]<seq and seq%7==0:
                    proc.stdin.write(last[2]);inserted+=1
                last=(table,seq,raw)
        proc.stdin.close();text=proc.stdout.read();assert proc.wait()==0
    r=json.loads(text)['data'];return dict(mode=mode,source_sha256=digest.hexdigest(),events_sent=sent+inserted,
                                          duplicated_or_late=inserted,dropped_public_actions=removed,**r)


if __name__=='__main__':
    path=Path(sys.argv[1]);size=path.stat().st_size
    assert hashlib.sha256(BINARY.read_bytes()).hexdigest()==FROZEN
    rows=[run(path,size,m) for m in ('clean','duplicates_and_reordering','sparse_history')]
    keys=('legal_decisions','postflop','missing_view')
    match=all(all(r[k]==rows[0][k] for k in keys) for r in rows)
    assert len(set(r['source_sha256'] for r in rows))==1
    report=dict(binary_sha256=FROZEN,capture_prefix_bytes=size,matching_decision_counts=match,cases=rows)
    OUT.mkdir(parents=True,exist_ok=True);(OUT/'recovery.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
    if not match or rows[0]['missing_view']:raise SystemExit('Replay discrepancy; see recovery.json')
