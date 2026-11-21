# Promotion benchmark

The target is sustainable **net BB/100**, measured from settled hand profits. Count every hand dealt, including folds; exclude grants, rebuys, and off-table transfers. Report a hands-won percentage and drawdown alongside it, but neither alone selects the policy. A hand share can be won while losing chips overall.

## Test design

- Separate training deals, tuning deals, and final evaluation deals. The evaluator plays the candidate once in each of the six seats on a shared deal. Compute uncertainty over whole deals, not hands treated as independent. Confidence intervals here are approximate, pointwise intervals, not guarantees about a changed public pool.
- Hard opponent: the local `history-compact-v1` checkpoint with its native pressure guard, minimum 64 visits and `equity-guarded-v3` fallback, without persistent player profiles. Reference checkout `99cb544129ee36a8739b8c02d5539028cd2bc041`; checkpoint SHA-256 `f3ed3b73624edfa43568e1248abe80b61deeb320f217e6d9a123d32879f9b6ca`. This is an offline proxy for the incumbent, not its complete live environment.
- Against five identical references, a reference in the sixth seat has exactly zero average return over a full seat rotation with identical per-seat randomness. Against a mixed field, actually play a second game with the reference in the candidate seat. Report both raw candidate BB/100 and **candidate minus reference**.
- Compare fixed 50, 100 and 200 BB starting stacks and a mixed-depth lineup. The mixed-depth test shuffles `[35, 60, 100, 150, 250, 500]` BB by deal. Stacks reset each hand; the test does not simulate an entire live bankroll path. Two simple deterministic opponents in the mixed field use a different implementation from Turbo's training arena.
- Before a live handover, look for positive chip EV at the default depth, no clear losing depth, and no clear paired regression in the mixed lineup. If those fail, keep the live champion and continue offline. A successful offline result merits a measured live trial, not a claim of established live profitability.

## Development history

The pot-frequency candidate (profile 10) won 23.38% but lost **37.8 bb/100** against the five trained references on 6,000 held-out 100-BB hands. The first profit-trained candidate (profile 17) learned 24 preflop overrides from 275,157 paired decisions on 400,000 synthetic deals. A separate 50,400-hand evaluation against trained references gave +8.82 bb/100 at 50 BB, **−22.34 bb/100 at 100 BB**, and +3.02 bb/100 at 200 BB. This is why winning against the scripted controls alone does not trigger deployment.

Profile 18 adds a small table of preflop and postflop overrides learned from paired continuations against the trained checkpoint; unseen or insufficiently supported states use profile 17. No opponent identity, private opponent cards, future board, secret or trace file is needed at inference time.

That unrestricted policy illustrated a failure of optimizing pot count: at 200 BB it won **87.5% of hands and lost 71.3 bb/100**. Its results are archived under `iterations/`. The training rollouts changed one action, while the unrestricted policy could change several, including escalating a hand it had already reopened. Subsequent candidates cap speculative exposure and limit the learned response to once per hand. A one-million-deal cross-population screen tested a more restrictive eight-bucket version (19). The one-response version with all supported response buckets (20) was selected on separate development seeds; neither those deals nor the unrestricted candidate's holdout are its final test.

The release test uses 3,500 matched deal blocks per fixed depth (seeds starting at 190250001), 3,800 empirical-depth blocks in the mixed lineup (160903001), and 2,500 100-BB blocks against an unseen checkpoint (213134001). The latter is `history-sustained-001`, with `equity-v1`; its SHA-256 is `4e606ddc39214b413fe7df48f66c3781c1a0413f9d2af74ad6e6a89fe800bcb8`. The mixed test replays an incumbent control as well as the candidate. `live-depths.json` contains anonymous aggregate stack lineups from preflop snapshots on the owner's tables; no identities or private cards are exported.

## Initial release decision: profile 20 (historical)

**Passed the initial offline benchmark.** Following the short public trial, the incumbent was restored and Turbo taken offline at the owner's request. See the [extended offline suite](extended/README.md). All figures below are simulated virtual-chip returns, not Turbo's results on the public leaderboard.

| Six-handed field | Candidate hands | Net BB/100 | 95% interval | Hands won |
| --- | ---: | ---: | --- | ---: |
| Trained reference, 50 BB | 21,000 | +93.8 | [+83.3, +104.3] | 63.6% |
| Trained reference, 100 BB | 21,000 | +83.6 | [+70.6, +96.6] | 60.9% |
| Trained reference, 200 BB | 21,000 | +100.2 | [+82.9, +117.5] | 61.3% |
| Mixed field, empirical stacks | 22,800 | +114.0 | [+97.9, +130.1] | 56.0% |
| Unseen checkpoint, 100 BB | 15,000 | +46.8 | [+29.4, +64.2] | 57.4% |

In the mixed trial the reference earned +45.9 BB/100 in the same candidate seats. The paired advantage was **+68.1 BB/100**, 95% interval **[+45.2, +91.1]**. The 100,800 count includes only candidate hands; the extra 22,800 matched control hands are not added. Repeated copies of any static strategy are exploitable; actual platform opponents, stacks and adaptations differ. Review live results separately with `performance` and the release's observed hand IDs.

Machine-readable outputs: `reference.json`, `mixed.json`, `unseen.json`, `arena.json`, `replay.json`. `holdout.html` draws the paired differences and their intervals. The pre-handover account history (not Turbo) showed +82.4 BB/100 over 9,000 settled public hands from November 16–21, 2026, with an approximate 100-hand-block interval of [+30.0, +134.8] and maximum drawdown of 1,340 BB. It includes prior policy versions.
