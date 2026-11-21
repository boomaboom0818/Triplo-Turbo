# Extended, offline-only validation of Turbo 20

[Completed results and downside analysis](RESULTS.md) · [Paired performance chart](intervals.html)

The incumbent was restored on 2026-11-21. The candidate is the exact binary from the short public trial (SHA-256 `6d305df121dfc99563e28b7895dbdda822108a57712005f90bd353872b97eb10`). No training, parameter selection or deployment is part of these experiments.

Test design before examining results:

- **Breadth:** 2–6 seats, frozen equity and mixed/scripted fields, three disjoint seed windows per condition, 8,500 duplicate-deal blocks per window. 1,020,000 candidate hands. These controls exercise coverage; high winnings here are not evidence about the public pool.
- **Independent-engine challenges:** repeat a reference/mixed test at live-like depths; face short stacks, very deep stacks, new opponent mixes, and a second checkpoint. Include a 100 BB common-menu sensitivity test so a result cannot be attributed solely to Turbo's finer choice of bet sizes. Compare against an actual incumbent control in the same seat against the same field. Report the paired BB/100 difference and uncertainty over independent deals.
- **Session risk:** play consecutive hands with rotating button and stacks carried between hands. Run paired candidate/control sessions on the same deal sequences, with rebuys recorded as funding rather than earnings. Measure peak-to-trough loss, losing-session frequency and busts over a fixed horizon. Treat sessions, not hands, as independent samples.
- Evaluate the full declared suite, including adverse conditions; do not select the best seed or promote automatically. Net BB/100 and paired difference are primary; percent won is descriptive. Statistical uncertainty across these simulated pools is separate from uncertainty about which opponents will actually play live.

The hard suite fixes six 6,000-block scenarios plus two sets of 300 independent 240-hand sessions. The primary fixed-stack scenario is `empirical_archive_mix`. Use simultaneous 95% Bonferroni intervals on the eight paired differences to screen for a material regression (upper endpoint below −5 BB/100). Flag session risk if Turbo's bust-session rate exceeds the control's by 10 percentage points or its 90th-percentile drawdown exceeds the control's by 50%. These are offline screening thresholds, not guarantees. The small runner smoke used seed offset zero; the scored suite uses offset 17,000,000.

### Reproduce

Set `TURBO_FROZEN_BINARY` to the original profile-20 executable. By default the scripts use `runs/frozen/profile-20` if present, otherwise `target/release/triplo-turbo`; they verify its hash before scoring. The reference venv and checkpoints are needed for the independent-engine tests. These commands never load an Open Poker key or open a gameplay socket:

```bash
ref="$HOME/Triplo-Texas-Holdem-Poker-"
python3 scripts/validation_breadth.py
"$ref/.venv/bin/python" scripts/validation_challenge.py "$ref" --workers 8
"$ref/.venv/bin/python" scripts/validation_configured.py "$ref"
"$ref/.venv/bin/python" scripts/validation_river.py "$ref"
"$ref/.venv/bin/python" scripts/validation_rules.py "$ref"
python3 scripts/validation_recovery.py "$ref/runs/bot/openpoker-season-17-equity/trace.jsonl"
python3 scripts/validation_terminal.py "$ref/runs/bot/openpoker-season-17-equity"
python3 scripts/validation_report.py
```

The 20 and 400 BB pressure tables mix two incumbent checkpoint seats, an archived checkpoint, an aggressive bot and a shover. The live-depth and 1,000 BB mixes use one incumbent checkpoint, two archive checkpoints, a station, and an aggressive bot. Roles are shuffled relative to hero by deal; control hero always uses the incumbent checkpoint with its usual fallback. In common-menu mode, both sides get the reference's raise sizes; off-menu Turbo proposals are rounded to the closest available raise, ties rounded down. The two 100 BB modes reuse deals deliberately. In carry-over mode lineups persist, the button rotates, and a zero stack receives 100 BB; there is no bankroll cap or top-up after a partial loss. The adaptive scenario uses two opponents that can widen their response after observing frequent preflop raises. There is no shared hidden information.

River correctness probes compare against exact heads-up equity over 990 blocker-compatible holdings. The PokerKit cross-check compares the challenger benchmark's independent game engine against a third-party rules implementation on arbitrary legal sizes, short blinds, and unequal stacks. These synthetic oracle deals are not EV samples. The recovery test reads one fixed prefix of the owner's redacted player trace three ways: unchanged, with duplicated/delayed events and reversed resync history, and with additional dropped public actions. It compares legal-decision counts and missing views, not the EV or exact action choices of a bot with incomplete information. No private hands or identifiers are exported.

The optional historical terminal-decision audit is distinct from synthetic performance testing. It considers only pre-trial, two-player call/fold offers where betting ends with that action; it checks both settings of the learned-response budget before joining the old client's recorded results. If Turbo would fold where the incumbent called, its counterfactual ending stack is known. If Turbo would call a fold, the return is unknown: report bounds, not invented showdowns. This describes a small, selected set of old-policy decision points, not how Turbo would have reached or played an entire live session.

A supplementary check mirrors the confirmed old service configuration: start at 50 BB, use the old client's actual decision adapter, its checkpoint/guard and its postflop sizing fit with a 20-opportunity threshold. It takes 200 fresh 300-hand sessions, randomly ordered lineups of two incumbent seats, two adaptive rule opponents and a station around hero. Positions rotate, busted seats re-enter at 50 BB, and statistical fits start empty and evolve from that branch's public observations only. Complete history is supplied; real account profiles, season balance/cooldowns, network delivery and private opponent cards are not supplied. This exploratory check is separate from the family-adjusted eight-test suite.
