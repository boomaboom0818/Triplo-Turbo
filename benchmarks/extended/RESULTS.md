# Extended evaluation: Turbo profile 20

**Deployment: incumbent live; Turbo offline.**

**Recommendation: keep the incumbent live.** The prespecified material-loss and downside thresholds were not breached. However, the incumbent beats Turbo under 100 BB · common sizing, 240-hand sessions · adaptive mix; the 50 BB deployment-configuration test does not establish a Turbo advantage.

Played **1,440,000 additional candidate hands** and **420,000 matched incumbent control hands**. The 1,020,000 native-arena hands test table-size and seed sensitivity. The 360,000 prespecified independent-engine hands and 60,000 supplementary deployment-configuration hands compare strength and session risk. Every result uses the same frozen executable.

| Condition | Turbo hands | Turbo BB/100 | Incumbent BB/100 | Paired difference [simultaneous 95% interval] |
|---|---:|---:|---:|---:|
| 20 BB · shove / pressure | 36,000 | +17.8 | -3.5 | +21.3 [+14.8, +27.9] |
| 100 BB · full sizing | 36,000 | +86.7 | +0.0 | +86.7 [+73.0, +100.5] |
| 100 BB · common sizing | 36,000 | -13.0 | +0.0 | -13.0 [-25.6, -0.5] |
| Live-like depths · archive mix | 36,000 | +96.5 | +26.2 | +70.3 [+46.2, +94.4] |
| 400 BB · pressure | 36,000 | +92.8 | +34.0 | +58.8 [+19.9, +97.8] |
| 1,000 BB · archive mix | 36,000 | +308.4 | +176.2 | +132.3 [+33.0, +231.5] |
| 240-hand sessions · archive mix | 72,000 | +108.8 | +38.9 | +69.9 [+47.5, +92.4] |
| 240-hand sessions · adaptive mix | 72,000 | +40.6 | +62.0 | -21.5 [-42.9, -0.1] |

## Session downside

Each distribution covers 300 **independent sessions**, 240 consecutive hands per session. Drawdown is measured on cumulative table profit, excluding rebuy grants. The two paths share the exogenous deals; bets, subsequent stack sizes and opponent responses can diverge.

| Field and player | Losing sessions | Sessions with a bust | Total busts | P90 / largest drawdown (BB) |
|---|---:|---:|---:|---:|
| carry_mixed · candidate | 14.3% | 32.3% | 124 | 231.1 / 922.0 |
| carry_mixed · control | 44.0% | 53.0% | 261 | 351.0 / 1017.5 |
| carry_adaptive · candidate | 38.3% | 45.7% | 199 | 283.9 / 626.5 |
| carry_adaptive · control | 36.7% | 52.3% | 239 | 335.2 / 583.5 |

## Supplement: deployed buy-in and incumbent opponent modeling

Actual restore settings: 50 BB buy-in, base-guard checkpoint, minimum 64 visits, equity-guarded-v3, opponent statistics on. Across 200 independent 300-hand sessions at 50 BB start/re-entry, Turbo earned **+34.8 BB/100** vs the incumbent control's **+29.6**, paired delta **+5.2** (pointwise 95% session interval [-6.8, +17.1]). Bust-session rates: 62.5% vs 69.5%; 90th-percentile drawdowns: 252.0 vs 258.2 BB. This separate estimate is not pooled with the eight prespecified comparisons. The incumbent decision adapter builds opponent profiles from public actions within each simulated session, not from identifiers or historical profiles on the real account.

## Interpretation

- The primary live-depth mixed field returned **+70.3 BB/100 relative to the incumbent**. 6 of 8 paired intervals lie entirely above zero. Material downside screens (whole interval below −5): none.
- Session risk flags on the predeclared thresholds: none. Losing individual sessions remains possible even in a positive-return field.
- The native arena had a positive point estimate in 30 of 30 runs (2–6 seats, 3 disjoint seeds per condition). The cheap scripted opponents can be very weak. These returns were not pooled with checkpoint results.
- River oracle: 9,142 decisions, including 6,014 all-in calls with an independently established unbeatable hand; **0 invariant violations**. For uniform heads-up ranges, equity-estimate RMSE was 1.70 percentage points. Oracle probes are not counted as hands.
- Simulator audit (PokerKit 0.7.7): 12,000/12,000 complete trajectories and payouts matched. 11,712 arbitrary raises outside the discrete menu were exercised; first-discrepancy categories: none. These are rules cases, not profit samples.
- Recorded-traffic replay: 17,096 decisions on the clean trace; 61,483 duplicate/late frames in one pass and 75,771 removed public actions in another. Matching decision counts: True; missing views: [0, 0, 0]. This is reducer-level evidence, not proof that different histories lead to identical strategies.
- Historical terminal decisions: 146 confirmed heads-up call/fold spots reached by the old live policies. Turbo would fold 6 actual calls (realized difference: -179.7 BB total) and call 2 actual folds (unknown returns). The overall logical bounds in this selected subset are [-388.1, 244.2] BB. This does not identify a live win rate; it suggests a targeted review of expensive folds.
- All intervals are approximate. The challenge chart/table adjusts for eight comparisons using independent deal blocks or sessions. Static checkpoints, a stylized adaptive opponent and unbounded virtual-chip re-entries do not reproduce the entire public pool. These tests do not establish live profitability or authorize a deployment.

Data: `breadth.json`, `challenge.json`, `configured.json`, `river.json`, `rules.json`, `recovery.json`, `terminal.json`. The challenge file also contains pointwise intervals, the two half-sample estimates per condition, the number of common-menu size translations, and counts of adaptive responses. See `README.md` for the trial design.
