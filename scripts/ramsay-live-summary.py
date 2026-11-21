"""Read only Ramsay's current service invocation and reconcile completed hands."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import subprocess
import tempfile

UNIT = 'triplo-turbo-profile21.service'  # stable unit name; profile is read from decisions


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--profit', action='store_true', help='also match hand IDs against Ramsay’s Open Poker history')
    a = p.parse_args()
    if subprocess.run(['systemctl', '--user', 'is-active', '--quiet', UNIT]).returncode:
        p.error('Ramsay is not running')
    invocation = subprocess.check_output(['systemctl', '--user', 'show', UNIT, '-p', 'InvocationID', '--value'], text=True).strip()
    if not invocation:
        p.error('Ramsay is not running')
    journal = subprocess.check_output(['journalctl', '--user', '-u', UNIT, f'_SYSTEMD_INVOCATION_ID={invocation}',
                                       '-o', 'cat', '--no-pager'], text=True)
    started, hands, profiles = set(), set(), set()
    decisions = {}
    acknowledgments = set()
    guarded = set()
    counts = Counter()
    for line in journal.splitlines():
        try:
            row = json.loads(line)
        except ValueError:
            continue
        if row.get('v') != 1:
            continue
        event, data = row['event'], row['data']
        hand = data.get('hand')
        counts[event] += 1
        if event == 'hand_start' and hand and isinstance(data.get('seat'), int):
            started.add(hand)
        elif event == 'hand_result' and hand in started and isinstance(data.get('seat'), int):
            if str(data['seat']) in (data.get('final_stacks') or {}):
                hands.add(hand)
        elif event == 'decision' and data.get('id'):
            profiles.add(data['profile'])
            decisions[data['id']] = hand
            if data.get('decision', {}).get('reason') == 'guarded_multiway_check':
                guarded.add(data['id'])
        elif event == 'ack' and data.get('id'):
            acknowledgments.add(data['id'])

    result = dict(profiles=sorted(profiles), complete_hands=len(hands), decisions=len(decisions),
                  acknowledged_actions=len(acknowledgments & decisions.keys()), rejections=counts['rejected'],
                  reconnects=max(0, counts['connected']-1), multiway_checks=len(guarded & acknowledgments),
                  busts=counts['busted'], server_errors=counts['server_error'])
    if a.profit and hands:
        if len(profiles) != 1:
            p.error('no single decision profile was observed in this invocation')
        home = Path.home()
        pid = int(subprocess.check_output(['systemctl', '--user', 'show', UNIT, '-p', 'MainPID', '--value'], text=True))
        installation = home / '.local/lib/triplo-turbo'
        exe = Path(f'/proc/{pid}/exe').resolve(strict=True)
        if not exe.is_relative_to(installation) or exe.name != 'triplo-turbo':
            p.error('Ramsay executable did not resolve to its installation')
        temp_dir = Path('/tmp/opencode')
        with tempfile.NamedTemporaryFile(mode='w', dir=temp_dir if temp_dir.is_dir() else None) as ids:
            ids.write('\n'.join(sorted(hands)))
            ids.flush()
            env = dict(os.environ, OPENPOKER_KEY_FILE=str(home/'.config/triplo/turbo-profile21.key'))
            result['profit'] = json.loads(subprocess.check_output([exe, 'performance', '1000', ids.name], env=env, text=True))
    print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
