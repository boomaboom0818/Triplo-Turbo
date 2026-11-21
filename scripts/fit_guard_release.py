"""Fit selective multiway checks from Turbo-only paired terminal outcomes."""
import hashlib
import json
import math
from pathlib import Path
import statistics as st
import subprocess

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / 'target/release/triplo-turbo'
OUT = ROOT / 'benchmarks/guard/release'
MODEL = ROOT / 'models/guard-release.json'


def run(*args):
    return json.loads(subprocess.check_output([BIN, *map(str,args)], cwd=ROOT))


def dump(path, obj):
    path.write_text(json.dumps(obj, indent=2) + '\n')


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    fitted = run('arena-fit-guard', 600, 240, 2_070_000_000)
    n_bb = fitted['bb']
    enabled, cells = [], []
    for bucket in range(8):
        by_field = [[(n, chips/n_bb) for n,chips in (row[bucket] for row in f['sessions'])] for f in fitted['fields']]
        counts = [sum(n for n,_ in group) for group in by_field]
        means = [sum(v for _,v in group)/max(n,1) for group,n in zip(by_field,counts)]
        pairs = [entry for group in by_field for entry in group]
        n = sum(counts)
        mean = sum(v for _,v in pairs)/max(n,1)
        se = st.stdev(v-mean*k for k,v in pairs)*math.sqrt(len(pairs))/max(n,1)
        lower = mean-2.734*se
        if min(counts)>=100 and min(means)>=0 and lower>0.10:
            enabled.append(bucket)
        cells.append(dict(bucket=bucket, pairs=counts, field_means_bb=means, pooled_mean_bb=mean, lower_bb=lower))
    dump(OUT/'fit.json',dict(fields=[f['field'] for f in fitted['fields']], cells=cells, enabled=enabled,
         seed=2_070_000_000, hands=600*240*len(fitted['fields'])))
    print('Enabled guard-release buckets:',enabled,flush=True)
    if not enabled:
        print('No qualifying change; did not run a redundant confirmation.',flush=True)
        return
    dump(MODEL,enabled)
    subprocess.run([ROOT/'scripts/cargo','build','--locked','--release'],check=True,cwd=ROOT)
    confirm = run('arena-lab', 432, 240, 2_180_000_000, 27)
    deltas = [r['paired_deltas_bb100']['27-22'] for r in confirm['fields']]
    confirm.update(binary_sha256=hashlib.sha256(BIN.read_bytes()).hexdigest(), candidate=27,
        offline_profit_gate=all(d['mean']>=0 and d['family_ci95'][0]>=-5 for d in deltas))
    dump(OUT/'confirmation.json',confirm)
    for row,d in zip(confirm['fields'],deltas):
        print(row['field'],round(d['mean'],2),[round(x,2) for x in d['family_ci95']],flush=True)


if __name__ == '__main__':
    main()
