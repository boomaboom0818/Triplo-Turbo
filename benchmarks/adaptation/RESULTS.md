# Profile 21: does the preflop change pay?

**Offline only.** The old model remains the public player. See the [trial design](README.md).

Confirmation: **378,000 hands per policy**, 1,134,000 in total. Each comparison pairs complete sessions on shared exogenous deals. The candidate was frozen before this seed range.

| Field | Candidate | Frozen 20 | Incumbent | Δ vs 20 [95%] | Δ vs incumbent [95%] |
|---|---:|---:|---:|---:|---:|
| Reactive · 100 BB | +66.3 | +54.3 | +55.3 | +12.0 [+5.9, +18.2] | +11.0 [-3.9, +25.8] |
| Configured · 50 BB | +37.5 | +21.9 | +32.8 | +15.6 [+10.7, +20.4] | +4.7 [-5.4, +14.8] |
| Mixed · live-like depths | +115.5 | +115.6 | +27.6 | -0.2 [-5.7, +5.4] | +87.9 [+73.6, +102.2] |
| Static · 100 BB | +87.4 | +81.6 | +45.4 | +5.8 [+0.3, +11.3] | +42.0 [+27.2, +56.7] |
| Unseen price-sensitive · 100 BB | +50.8 | +19.7 | +72.8 | +31.1 [+20.7, +41.4] | -22.0 [-40.9, -3.2] |

All values above are **net BB/100**. The intervals above are pointwise. To read all ten comparisons together, use the simultaneous 95% intervals below (Bonferroni normal approximation, session-clustered). An interval containing zero does not resolve which policy is better.

| Field | Δ vs frozen [family 95%] | Δ vs incumbent [family 95%] |
|---|---:|---:|
| Reactive · 100 BB | +12.0 [+3.2, +20.8] | +11.0 [-10.3, +32.2] |
| Configured · 50 BB | +15.6 [+8.6, +22.5] | +4.7 [-9.8, +19.2] |
| Mixed · live-like depths | -0.2 [-8.1, +7.8] | +87.9 [+67.4, +108.3] |
| Static · 100 BB | +5.8 [-2.1, +13.7] | +42.0 [+20.8, +63.1] |
| Unseen price-sensitive · 100 BB | +31.1 [+16.2, +45.9] | -22.0 [-49.1, +5.0] |

## Downside and behavior

90th-percentile peak-to-trough drawdown is in big blinds over a 240-hand session (300 hands at 50 BB). Bust percentage is the share of sessions with at least one re-entry.

| Field | Player | P90 drawdown | Bust sessions | Won | Preflop raises per 100 hands¹ |
|---|---|---:|---:|---:|---:|
| Reactive · 100 BB | Candidate | 238.1 | 34.3% | 46.7% | 70.8 |
| Reactive · 100 BB | Frozen | 256.9 | 41.7% | 47.6% | 77.7 |
| Reactive · 100 BB | Incumbent | 311.1 | 47.3% | 18.8% | 31.1 |
| Configured · 50 BB | Candidate | 239.1 | 60.3% | 44.1% | 66.0 |
| Configured · 50 BB | Frozen | 253.7 | 68.7% | 44.6% | 71.7 |
| Configured · 50 BB | Incumbent | 284.1 | 65.0% | 19.0% | 28.6 |
| Mixed · live-like depths | Candidate | 225.7 | 33.3% | 52.5% | 73.7 |
| Mixed · live-like depths | Frozen | 238.6 | 32.3% | 53.7% | 76.5 |
| Mixed · live-like depths | Incumbent | 367.4 | 55.0% | 17.0% | 29.6 |
| Static · 100 BB | Candidate | 241.1 | 27.7% | 53.3% | 76.0 |
| Static · 100 BB | Frozen | 240.0 | 32.0% | 53.9% | 78.8 |
| Static · 100 BB | Incumbent | 331.7 | 52.3% | 18.9% | 30.8 |
| Unseen price-sensitive · 100 BB | Candidate | 277.5 | 48.3% | 40.2% | 64.7 |
| Unseen price-sensitive · 100 BB | Frozen | 350.4 | 67.3% | 39.3% | 74.8 |
| Unseen price-sensitive · 100 BB | Incumbent | 396.6 | 57.7% | 16.4% | 30.8 |

¹ Counts hands with at least one hero raise; never more than one per hand. Percent won is descriptive, not the selection objective.

## Diagnosis and fit

| Field | Weak opens | Fold-through | Reraised | Net BB per weak open |
|---|---:|---:|---:|---:|
| Reactive · 100 BB | 13,148 | 33.8% | 17.8% | -0.14 |
| Static · 100 BB | 13,685 | 42.8% | 10.7% | +0.51 |
| Mixed · live-like depths | 12,967 | 43.4% | 9.2% | +0.67 |

Fit: 120,000 baseline training hands, 170,713 full-hand alternative trajectories, 4 enabled buckets. The first simple tighten-up trial did not improve on profile 20 in the target 100 BB field. The fitted candidate chooses among check/fold, check/call and capped sizes using public evidence; a bucket without sufficient training or recent support retains the original decision. When the alternative changes the action it consumes the same once-per-hand response budget.

Actual changes on the held-out sessions:

| Field | Check/fold | Cheap call | Resized raise |
|---|---:|---:|---:|
| Reactive · 100 BB | 5,714 | 0 | 5,825 |
| Configured · 50 BB | 7,001 | 0 | 6,732 |
| Mixed · live-like depths | 1,943 | 0 | 5,334 |
| Static · 100 BB | 2,180 | 0 | 5,715 |
| Unseen price-sensitive · 100 BB | 9,670 | 0 | 5,961 |

Candidate binary: `d29e36678097424aa8667f8b077edde018cc67685d37ead46e3c19797bbb4857`. Frozen comparator: `6d305df121dfc99563e28b7895dbdda822108a57712005f90bd353872b97eb10`.

These are simulated fields and table-local opponent histories. They measure sample uncertainty within these matchups, not uncertainty about the makeup of a real public table. The old agent’s cross-session real-player profiles are not imported.
