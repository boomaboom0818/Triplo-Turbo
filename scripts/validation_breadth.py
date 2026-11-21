"""Seat-count/seed sensitivity for the frozen candidate; no live I/O."""
import hashlib
import json
import os
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'benchmarks/extended'
BINARY = Path(os.environ.get('TURBO_FROZEN_BINARY', ROOT / 'runs/frozen/profile-20'))
if not BINARY.exists() and 'TURBO_FROZEN_BINARY' not in os.environ:
    BINARY = ROOT / 'target/release/triplo-turbo'
FROZEN = '6d305df121dfc99563e28b7895dbdda822108a57712005f90bd353872b97eb10'


if __name__ == '__main__':
    assert hashlib.sha256(BINARY.read_bytes()).hexdigest() == FROZEN
    OUT.mkdir(parents=True, exist_ok=True)
    rows = []
    for repeat in range(3):
        for seats in range(2, 7):
            for j, field in enumerate(('baseline', 'mixed')):
                seed = 700_000_000 + 1_000_000*repeat + 20_000*seats + 10_000*j
                cmd = [BINARY, 'arena', '8500', str(seats), str(seed), '512', '20', field]
                row = json.loads(subprocess.check_output(cmd, env={**os.environ, 'RAYON_NUM_THREADS': '4'}))
                assert row['hands'] == 8500*seats and row['seed'] == seed
                rows.append(row)
    (OUT / 'breadth.json').write_text(json.dumps(dict(binary_sha256=FROZEN, trials=rows), indent=2)+'\n')
    print('Completed', sum(r['hands'] for r in rows), 'candidate hands across', len(rows), 'trials')
    placebo=[]
    for seats in range(2,7):
        seed=978_000_000+seats*2000
        r=json.loads(subprocess.check_output([BINARY,'arena','1024',str(seats),str(seed),'512','0','baseline'],
                                             env={**os.environ,'RAYON_NUM_THREADS':'2'}))
        assert r['hero_bb_per_100']==0 and r['block_95pct_ci']==[0,0]
        placebo.append(dict(seats=seats,hands=r['hands'],seed=seed,bb100=r['hero_bb_per_100'],interval=r['block_95pct_ci']))
    (OUT/'placebo.json').write_text(json.dumps(dict(binary_sha256=FROZEN,scope='Bookkeeping control only: identical profile 0 in every seat',cases=placebo),indent=2)+'\n')
