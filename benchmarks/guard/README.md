# Ramsay: low-equity postflop guard

This trial uses only Turbo's code, its own prior policy, synthetic opponents and one regression from ramsay's live decisions. No other operator's bot, logs, repository, or checkpoint is required or used.

In one live hand, profile 21's base policy would have checked, but a learned response replaced that check with a 10.5 BB bet. Four opponents remained, the board was paired, and the estimate under the bot's range model was below 10%. Ramsay folded to a raise. One hand is not a causal estimate or evidence of long-run profitability.

Profile 22 vetoes a learned bet when a check was the original action, at least two opponents remain, and estimated equity is below 25%. It keeps the check without spending the override budget. It does not directly modify preflop, heads-up, base value bets, or decisions facing a bet. Subsequent decisions can differ because the pot and remaining override budget changed. The threshold is fixed at the existing lowest equity bucket boundary, not selected on confirmation data.

The Rust simulator uses matched deck and per-seat RNG seeds, rotating positions, carry-over stacks, and chips supplied on bust separately from hand profit. Scripted fields include mixed, price-sensitive and calling-heavy opponents at 50 BB, plus opponents shifting their defense based on observed opens at 100 BB. Both profiles use the same public opponent-history reducer and see no roles or opponent cards. Compare session-level net BB/100, bust frequency and 90th-percentile peak-to-trough drawdown. Both pointwise and family-adjusted 95% intervals are approximate; these synthetic fields cannot establish live profitability. The installed binary is independent of testing and is not updated by these commands.

## Results

Confirmation played 345,600 hands per policy, 691,200 in total. Profile 22 checked instead of injecting a learned bet 31,529 times. In the four fields, 90th-percentile drawdown and bust-session rates were lower. The profitability intervals all cross zero once adjusted for four comparisons. Profile 22 was enabled on ramsay at the owner's request, not because these estimates established a profit gain. Its installed SHA-256 is `ba5681237b0a9396129e400eeb0320f7c615e7626d13fb4ad06d09c7859455dd`.

| Field | Paired BB/100 change [simultaneous 95%] | P90 drawdown, 21 → 22 (BB) | Bust sessions, 21 → 22 |
|---|---:|---:|---:|
| Mixed, 50 BB | +11.1 [−3.4, +25.6] | 386.6 → 376.6 | 56.9% → 52.8% |
| Price-sensitive, 50 BB | +13.5 [−0.3, +27.4] | 340.0 → 322.4 | 55.6% → 53.3% |
| Calling-heavy, 50 BB | −12.0 [−35.1, +11.2] | 358.9 → 322.1 | 35.3% → 29.4% |
| Reactive, 100 BB | −4.7 [−21.5, +12.1] | 469.1 → 458.7 | 32.5% → 29.4% |

All 20 Rust tests passed. An additional 3,500 synthetic decision probes exactly matched the deployed binary when selecting profile 21; profile 22 changed only the 231 targeted spots in that probe set. [Machine-readable results](confirmation.json) · [Interval chart](intervals.html)

```sh
scripts/cargo test --locked --release
scripts/cargo build --locked --release
RAYON_NUM_THREADS=6 target/release/triplo-turbo arena-guard 360 240 1710000000
```

## Next iteration

Experiment 23 stops learned additions of weak early-position opens if the original action was check or fold. Experiment 24 scales the multiway equity floor by the number of opponents; 25 combines those two changes. Experiment 26 prevents a learned response from replacing a baseline unraised-pot value raise with a passive move on QQ, KK, AA or AK. All four still check in the original low-equity failure. The only opponent information these experiments use is what Turbo can observe during a hand and through its existing public action history.

`RAYON_NUM_THREADS=6 python3 scripts/ramsay_lab.py` selects by the worst matched development-field mean versus live profile 22, then uses fresh seeds and a sixth, previously unused lineup for confirmation. The offline gate requires all six matched means to be nonnegative versus profile 22 and all six simultaneous lower bounds to be at least −5 BB/100. This is a benchmark regression screen, not a promise of no losses or a deployment step.

Profile 24 was selected in development, but failed confirmation: against profile 22 it gained +18.5 BB/100 in the calling-heavy field and lost 7.5 BB/100 in the mixed field. Neither that pattern nor the small live sample is permission to switch. See [confirmation data](lab/confirmation.json).

The local rollout experiment (`RAYON_NUM_THREADS=6 python3 scripts/fit_guard_release.py`) forked each eligible hand once at the first spot where 22 checks and 24 bets. The two continuations shared the same deck and known pot, branched at that action, and finished the hand with the same policies. Payouts were grouped in eight observable bins (flop or later, effective stack below or above two pots, any opponent with a public raise in the hand). An exception required at least 100 forks per training field, no negative field mean, and a pooled session-clustered lower bound above 0.10 BB per decision. Across 720,000 training hands there were 10,215 eligible forks, but none of the eight bins cleared the screen. [Training evidence](release/fit.json).

The calling-heavy field was the consistent exception in those replays. Profile 28 tests whether actual opponents' recent actions can identify that case safely. It uses the relative equity floor only when at least half of the remaining opponents have enough public postflop history to qualify as frequent callers. Qualification requires 14 or more observed responses, counted once per opponent per hand in the rolling 80-hand window, and an 83% shrunken call rate among prices from 1/8 pot to pot. Raises count as non-calls. Missing evidence leaves profile 22's fixed floor intact. The result is tested on fresh deals with `RAYON_NUM_THREADS=6 target/release/triplo-turbo arena-lab 528 240 2280000000 28`. The tests identify seats with synthetic names; the policy does not see simulated opponent roles or private cards. No automatic deployment.

Profile 28 passed the previously defined offline profit gate on 760,320 fresh hands per policy. Four fields had identical outcomes and zero exceptions; the reactive field had one exception and a +0.16 BB/100 estimate (simultaneous interval [−0.29, +0.60]). The calling-heavy field had 3,817 exceptions and +24.50 BB/100 ([+10.15, +38.86]). In that field P90 drawdown rose from 377.05 to 380.35 BB and bust sessions from 37.50% to 37.88%. This supports a narrow change against reliably frequent callers, not general live profitability. Profile 22 remains live. [Confirmation data](sticky/confirmation.json) · [Interval chart](sticky/intervals.html).

For the live rollout, `python3 scripts/ramsay-live-summary.py --profit` reconciles only hands with a start and a result in Ramsay's current invocation against Open Poker's read-only hand-history API. It excludes any hand already in progress at connection. Without `--profit` it uses only the local journal. Neither mode touches gameplay or changes the running service.
