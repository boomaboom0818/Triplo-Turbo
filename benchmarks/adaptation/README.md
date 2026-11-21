# Preflop defense adaptation: offline experiment

The target is **net big blinds per 100 dealt hands** (BB/100 = 100 × net big blinds / hands), including blinds and folds. Re-entry funds never count as profit. The frozen before-fix control is the installed public-trial executable, SHA-256 `6d305df121dfc99563e28b7895dbdda822108a57712005f90bd353872b97eb10`. A second control is the incumbent. Only the 50 BB condition includes its actual wire decision adapter and within-session opponent statistics.

**Decision after confirmation: keep the incumbent live.** Profile 21 improved on frozen Turbo by 12.0 BB/100 in the primary reactive field and by 15.6 BB/100 at 50 BB. Both gains survived adjustment for all ten comparisons. The mixed-field edge over the incumbent was retained. The 50 BB result did not establish superiority over the incumbent, and the incumbent had the better point estimate in the unseen price-sensitive field. [Full results and session downside](RESULTS.md) · [Machine-readable comparison](confirm.json).

## Protocol fixed before confirmation

- Diagnosis: profile 20's first preflop raise, grouped by price to call (≤1 BB versus >1 BB) and by a descriptive hand ranking (Chen <7 / 7–<10 / ≥10). “Fold-through” means no other player subsequently called or raised preflop. These are selected-hand returns, not causal estimates of replacing the action. The deployed eligibility check is stricter: hero's current street bet **plus** the price to call must be ≤1 BB, excluding a big blind facing a minraise.
- Fit: use observed replies from the last 80 hands at the current table. Count only voluntary, priced responses directly to hero's first preflop raise, ending attribution at the first opposing reraise. Estimate a fold-through probability and a reraise probability from the current named opponents, distinguishing a player who already voluntarily invested from one who did not. Unknown players receive prior rates; sparse history does not change an action. Hole cards of opponents, their role labels, and the opponent simulator's frequency trigger are never policy inputs.
- Compare a simple tighten-up rule in development with alternatives fitted to full-hand counterfactual payouts. Fitting forks from a baseline session: identical cards, stacks, opponent seeds and hero decision seeds. Alternatives: check/fold, check/call, or raise to 5/9 BB if legal and within the exposure cap; no fitted action on premium hands or facing a raise. Learn per supported bucket only with ≥250 observations, mean paired gain minus two session-clustered standard errors >0.04 BB, and no training mixture mean below −0.25 BB. Rerun the complete selected policy on independent sessions: one-hand fits alone cannot establish its value after opponents respond or stacks drift.
- Development/diagnosis/fit/confirmation have separate seed namespaces: `1,200,000,000` / `1,100,000,000` / `1,400,000,000` / `1,300,000,000`. The confirmation candidate and model are fixed **before** running the last namespace. No retraining or selection on those results.
- Confirmation: **300 independent sessions per field**, one branch per policy, same rotating button, initial stacks, opponents, deck seeds and exogenous RNG per seat. Each branch independently evolves opponents' beliefs, hero's public image, stacks and re-entries. Each hand satisfies zero-sum payouts; each branch reconciles its net result with starting and ending balance minus rebuys. The units for uncertainty are entire paired sessions.
- Primary test: candidate minus profile 20 against the original reactive field at 100 BB. Also compare with the actual incumbent; a win over profile 20 alone does not establish that Turbo should replace the incumbent. Repeat at the deployed 50 BB settings (300 hands/session), empirical mixed depths, 100 BB with the adaptive responders replaced by their static base policy, and previously untested elastic defenders who respond to price rather than hero's raise frequency. Nonconfigured sessions last 240 hands. Report pointwise and simultaneous 95% intervals for the ten paired differences, 90th-percentile drawdown and bust rate. Keep all conditions, including adverse ones. No automatic deployment.

## Reproduce

The legacy validation scripts and the new runner accept `TURBO_FROZEN_BINARY` and verify the original fingerprint. The local default is `runs/frozen/profile-20` when present. The new binary is `target/release/triplo-turbo`.

```sh
ref="$HOME/Triplo-Texas-Holdem-Poker-"
export TURBO_FROZEN_BINARY="$HOME/.local/lib/triplo-turbo/live/triplo-turbo"
scripts/cargo build --release --locked
"$ref/.venv/bin/python" scripts/adaptive_sessions.py "$ref" diagnose --sessions 120 --fields reactive100 static100 mixed
"$ref/.venv/bin/python" scripts/fit_adaptive.py "$ref" --sessions 125
scripts/cargo build --release --locked
"$ref/.venv/bin/python" scripts/adaptive_sessions.py "$ref" confirm --sessions 300
python3 scripts/report_adaptive.py
```

The confirmation runner writes aggregated results plus `sessions.jsonl.gz` (one aggregate per matched session, no hole cards) so paired estimates and downside distributions can be recalculated. To exercise profile 21 on a recorded wire capture, use `triplo-turbo replay <wire.jsonl> 21`. In `policy-stdio`, profile 21 accepts a `history_session` ID and `completed_hands`, a list of new, chronological public event lists since the last request. Switching session IDs clears the rolling history. Current decisions still use their own `events`, `snapshot` and `response_used`. A request without a history session uses the cold-start policy.
