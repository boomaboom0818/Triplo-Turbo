# Ramsay: recovery and price-aware preflop play

## Live recovery

Ramsay's original live profile-22 process recorded a `busted` event, a `cannot_rebuy` error, then 34 `invalid player resync` disconnects. The account was off-table with 3,610 virtual chips available, enough for another 1,000-chip buy-in, so another rebuy was not the correct operation. The connector had retained the old table ID across reconnects.

The recovery build verifies the actual seat before each reconnect; it retains history if the table is unchanged and clears it if the seat disappeared or moved. It rejoins after a bust, requests a rebuy only after a low-balance response, and waits when an auto-rebuy or cooldown is pending. A local WebSocket test covers bust, an in-flight frame from the old table, a low-balance response, and cooldown recovery. The tested transport-only build was installed with profile 22 unchanged, at 50 BB, while no hand was in progress. It successfully seated at a new table and received action acknowledgments. Installed SHA-256: `0e64d8dac168db983e211af5da1746aecb9acdf6b1f14682b20e2bf5d92c3f69`.

## Offline strategy experiment

Forty-three complete pre-recovery hands lost 41.75 BB. In that selected sample, four first decisions where a rollout called an existing raise lost 16 BB, and six learned raises in unraised pots lost another 8.5 BB. These are descriptive outcomes, not expected values and not causal evidence that folding would be better across many sessions.

Profiles 29–31 replace the loose learned preflop actions with a price-aware cold strategy. Suitedness, effective depth, position, opponent count, amount already wagered, pot odds and pair strength decide when a raise, priced call or fold is justified. Premium hands can still raise or short-shove. Profile 30 is slightly wider than 29; profile 31 uses that wider fallback but may retain legacy late-position or short-handed steals when sufficient public history shows likely folds. All retain profile 28's postflop guard and only use their own legal public view. The installed profile is not changed by this experiment.

The arena compares against live profile 22 and profile 28 on matched 240-hand sessions with rotating positions, stack carry-over and rebuy funding excluded. Development independently selects by the worst profit improvement in the two stronger 50 BB fields, with overall mean as tiebreaker. Confirmation uses a different seed namespace, tests the selected policy once, and adds two unseen tough lineups (50 BB and 100 BB). Report the full six-field family and downside; a softer field with a large edge cannot compensate for a confirmed loss in a harder 50 BB matchup. Do not treat this toy arena's absolute returns as live projections.

```sh
scripts/cargo test --locked --release
scripts/cargo build --locked --release
RAYON_NUM_THREADS=6 python3 scripts/ramsay_strategy.py
```

No other operator's bot, account, logs, repository, or checkpoint was used.

The broader rewrite was rejected. Profile 31 was selected in development; on fresh deals it lost ground versus profile 28 in all six fields. The strongest unseen 50 BB test moved from +144.9 to −6.1 BB/100. Lower draws in several other fields did not justify that loss. See [matched confirmation](confirmation.json) and [paired intervals](intervals.html).

## Targeted experiment: weak, expensive calls

The audit narrowed the public losses to four specific rollout calls: 3–3.5 BB into a 5 BB pot, with weak or speculative hands. Profiles 32 and 33 keep profile 28's raises, value hands, and original postflop behavior. Profile 32 vetoes a learned non-premium call when the immediate price requires at least 34% equity and the hand-strength screen is weak. Profile 33 also vetoes some discounted calls requiring 25–34% equity when remaining effective stack is less than 12 times the call. Neither replaces a base-policy call. The experiment uses the same field definitions but disjoint seed ranges from the broad rewrite, with fresh independent confirmation after selection. Run `RAYON_NUM_THREADS=6 python3 scripts/ramsay_strategy.py --price`. Freeze the established baseline unless the data supports a change.

Unconditional profile 32 gained +1.83 BB/100 against profile 28 in the tough unseen 50 BB lineup, but lost 15.63 BB/100 against frequent callers (simultaneous 95% interval [−27.78, −3.48]). It was not deployed. [Targeted confirmation data](price/confirmation.json).

Profile 34 tests a narrower rule: gate the same veto on recent postflop evidence. At least half of the remaining opponents (minimum two) must have 12 or more observations and a shrunken call rate below or equal to 68%; any opponent already identified as a frequent caller blocks the veto. Unknown history is not evidence. The one fixed candidate is tested on fresh 240-hand sessions, including a seventh, unseen lineup containing three independent instances of Turbo's own prior guarded policy. Benchmark: `RAYON_NUM_THREADS=6 target/release/triplo-turbo arena-price 432 240 2850000000 34`. Negative results are retained; no live actions depend on this experiment.

Profile 34 did not improve on profile 28. It vetoed four calls across the first six fields and 1,619 in the Turbo-heavy lineup. The latter difference was −0.31 BB/100 (simultaneous 95% interval [−2.00, +1.38]); the other differences versus 28 were approximately zero. Its +20.61 BB/100 against live 22 in the calling-heavy field came from the existing profile-28 logic, not the new preflop rule. Keep the simpler profile 28 as the next live trial candidate, with the verified reconnect and rebuy fix. [Context confirmation](context/confirmation.json).

The local profile-28 candidate is staged at `runs/releases/profile28/triplo-turbo`, SHA-256 `9c99b84f5d4a9a62e2e4366500be7b9ec03dae3dc2ac575c4602795c654c2cbc`. The release command supports profile 28 explicitly; unvalidated profiles remain unavailable to the live command. All 27 Rust tests passed and 1,200 synthetic profile-28 decisions exactly matched the earlier implementation. Staging did not restart Ramsay or change its profile-22 service.

**Live trial:** At the owner's request, Ramsay switched to this exact profile-28 binary at 50 BB after a hand settled. It reconnected to its existing table; Open Poker acknowledged a profile-28 action and settled that hand. Profile 22's recovery binary remains installed separately for rollback. This verifies operation, not profitability.
