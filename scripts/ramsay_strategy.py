"""Compare price-aware preflop policies against Ramsay's prior profiles."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]
BIN = ROOT / 'target/release/triplo-turbo'
OUT = ROOT / 'benchmarks/strategy'


def digest():
    return hashlib.sha256(BIN.read_bytes()).hexdigest()


def run(phase, sessions, seed, profile, sha, cmd, out):
    data = json.loads(subprocess.check_output([BIN, cmd, str(sessions), '240', str(seed), str(profile)]))
    assert sha == digest()
    data.update(candidate=profile, binary_sha256=sha)
    (out/f'{phase}.json').write_text(json.dumps(data, indent=2)+'\n')
    return data


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--price',action='store_true',help='evaluate only the narrow learned-call veto')
    narrow=p.parse_args().price
    out=OUT/'price' if narrow else OUT
    cmd='arena-price' if narrow else 'arena-strategy'
    out.mkdir(parents=True, exist_ok=True)
    sha = digest()
    dev = run('development', 216 if narrow else 204, 2_650_000_000 if narrow else 2_450_000_000, 0, sha,cmd,out)

    # Prefer the best worst hard-field improvement; then use the mean across fields.
    def grade(p):
        vals = {f['field']: f['paired_deltas_bb100'][f'{p}-28']['mean'] for f in dev['fields']}
        return min(vals[f] for f in ('counter_50', 'value_pressure_50')), statistics.mean(vals.values()), -p

    selected = max(range(32,34) if narrow else range(29,32), key=grade)
    print('Selected profile',selected,flush=True)
    conf = run('confirmation', 432 if narrow else 360, 2_750_000_000 if narrow else 2_550_000_000, selected, sha,cmd,out)
    for f in conf['fields']:
        d=f['paired_deltas_bb100'][f'{selected}-28']
        print(f['field'],round(d['mean'],2),[round(x,2) for x in d['family_ci95']],flush=True)


if __name__ == '__main__':
    main()
