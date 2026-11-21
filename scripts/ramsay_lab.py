"""Select a candidate on development deals and confirm on fresh local deals."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'benchmarks/guard/lab'
BIN = ROOT / 'target/release/triplo-turbo'


def digest():
    with BIN.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def run(phase, sessions, seed, profile, sha):
    data = json.loads(subprocess.check_output([BIN, 'arena-lab', str(sessions), '240', str(seed), str(profile)]))
    assert digest() == sha
    data.update(candidate=profile, binary_sha256=sha)
    (OUT / f'{phase}.json').write_text(json.dumps(data, indent=2) + '\n')
    return data


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--development-sessions', type=int, default=216)
    p.add_argument('--confirmation-sessions', type=int, default=384)
    a = p.parse_args()
    OUT.mkdir(parents=True, exist_ok=True)
    sha = digest()
    dev = run('development', a.development_sessions, 1_810_000_000, 0, sha)

    # Maximize the worst matched field versus the current deployed baseline.
    # Break a tie with the average, then prefer fewer/older changes by profile ID.
    def score(profile):
        means = [r['paired_deltas_bb100'][f'{profile}-22']['mean'] for r in dev['fields']]
        return min(means), statistics.mean(means), -profile

    selected = max(range(23, 27), key=score)
    print(f'Development selected profile {selected}; confirming on fresh deals.', flush=True)
    confirm = run('confirmation', a.confirmation_sessions, 1_920_000_000, selected, sha)
    outcomes = [r['paired_deltas_bb100'][f'{selected}-22'] for r in confirm['fields']]
    passes = all(d['mean'] >= 0 and d['family_ci95'][0] >= -5 for d in outcomes)
    (OUT / 'decision.json').write_text(json.dumps(dict(profile=selected, offline_profit_gate=passes,
        worst_mean=min(d['mean'] for d in outcomes), worst_ci95_lower=min(d['family_ci95'][0] for d in outcomes)), indent=2)+'\n')
    for field, d in zip(confirm['fields'], outcomes):
        print(field['field'], round(d['mean'], 2), [round(x, 2) for x in d['family_ci95']], flush=True)


if __name__ == '__main__':
    main()
