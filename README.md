<img src="assets/icon.svg" width="64" height="64" alt="">

# Triplo Turbo

Self-hosted **Rust six-max No-Limit Hold’em** bot for [Open Poker](https://docs.openpoker.ai/platform/game-rules/), plus a local rules engine, seat-rotated arena, Bayesian range/HUD engine, and multi-population offline trainer. The same player-visible policy runs in simulation, trace replay, shadow observations, and public play. There is no Python process, remote model, or training job in the live decision path.

## Triplo Turbo V2 (Profile 40 — Current Release)

Profile 40 (`src/v2.rs`, `models/v2.json`) replaces the legacy heuristic + lookup-override pipeline with a self-contained Rust quantitative engine designed for 6-max 0%-rake No-Limit Hold'em:

- **Per-opponent Bayesian HUD (`src/hud.rs`)**: Tracks VPIP, PFR, limp%, 3-bet%, fold-to-open, fold-to-3bet, flop c-bet, postflop fold/call/raise vs bet, aggression, river aggression, and WTSD by persistent table name with Beta-Binomial shrinkage.
- **1,326-combo Bayesian range tracker (`src/range.rs`)**: 6-max 0%-rake 169-hand playability valuation and explicit `[f32; 1326]` opponent combo distributions updated street-by-street on every public action, with board-texture & 4-flush bucketing.
- **Range-vs-range EV-tree solver (`src/eval.rs`, `src/v2.rs`)**: Exact river enumeration (~20 µs) and stratified flop/turn Monte Carlo evaluating Fold, Check, Call, and multi-sizing Bets/Raises (geometric sizing + HUD-gated calling-station overbets) against pot-odds-elastic opponent response models.
- **Dynamic stack depth (`src/live.rs`)**: `live 5000 40` automatically scales table buy-ins up to 5,000 chips (250 BB) capped to available season `chip_balance` (minimum 1,000 chips).

### V2 Held-Out Confirmation (480,000 Fresh Hands)

Matched-seed 6-max carry-over-stack sessions (`150 sessions × 200 hands = 30,000 hands` per policy per environment, fresh seed namespace `8800000000`, `p99` decision latency `0.35–0.67 ms`; machine-readable results in [benchmarks/v2/confirmation.json](benchmarks/v2/confirmation.json)):

| Environment | Depth | V2 (Profile 40) BB/100 | 95% CI | Bust% | Profile 28 BB/100 | V2 − P28 [95% CI] |
|---|---:|---:|---:|---:|---:|---:|
| `openpoker_pool_250bb` | 250 BB | **+355.3** | [+299.3, +411.3] | 25.3% | +279.6 | **+75.7** [+23.8, +127.7] |
| `openpoker_pool_50bb` | 50 BB | **+126.8** | [+107.4, +146.1] | 55.3% | +87.9 | **+38.9** [+19.4, +58.3] |
| `tough_regs_100bb` | 100 BB | **+4.2** | [−9.5, +17.9] | 45.3% | −43.0 | **+47.2** [+35.5, +59.0] |
| `tough_regs_250bb` | 250 BB | **+2.9** | [−15.5, +21.2] | 18.0% | −57.2 | **+60.1** [+40.2, +80.1] |
| `maniac_station_250bb` | 250 BB | **+1,139.8** | [+893.2, +1,386.4] | 54.0% | +295.9 | **+844.0** [+600.5, +1,087.4] |
| `mixed_bots_250bb` | 250 BB | **+551.0** | [+462.7, +639.4] | 36.7% | +203.7 | **+347.3** [+264.6, +430.1] |
| `vs_profile28_50bb` | 50 BB | **+68.3** | [+61.3, +75.4] | 26.0% | +0.2 | **+68.1** [+59.0, +77.2] |
| `vs_profile28_250bb` | 250 BB | **+80.8** | [+62.0, +99.7] | 16.0% | −6.4 | **+87.3** [+67.5, +107.1] |

```bash
scripts/cargo test --release --locked
scripts/cargo build --release --locked
target/release/triplo-turbo arena-v2 150 200 8800000000 256   # 480k-hand benchmark
target/release/triplo-turbo tune-v2 55 48 180 6100000000      # multi-population trainer
OPENPOKER_KEY_FILE="$HOME/.config/triplo/openpoker.key" target/release/triplo-turbo live 5000 40
```

---

### Legacy notes

**Profile 21** is the offline candidate for opponents who defend preflop steals. It uses recent public replies at the current table. See the [adaptive-session experiment](benchmarks/adaptation/README.md) and [original benchmark](benchmarks/README.md). Profile 20 remains the default and frozen comparator; the old model holds the public seat.

**Deployment record (2026-11-21 10:09 UTC):** the incumbent `triplo-equity-live.service` was restored and enabled; its client subsequently recorded an action acknowledgment and a settled hand. `triplo-turbo-live.service` is stopped and disabled. See the [extended offline evaluation](benchmarks/extended/README.md). The brief Turbo trial logged 5 settled hands and 13 accepted actions, no rejections; the first hand may span the handover. That sample is too small to measure profitability. Installed candidate SHA-256: `6d305df121dfc99563e28b7895dbdda822108a57712005f90bd353872b97eb10`. Do not attribute account-wide results to Turbo.

## Run it

```bash
scripts/cargo build --release --locked
scripts/cargo test --release --locked
target/release/triplo-turbo arena 1000 6 123456
```

`scripts/cargo` selects the local Rust toolchain on this host. On other machines a recent stable Rust with a C toolchain and ordinary `cargo` works. For a **Self Host** bot, put the dashboard key in a mode-600 file outside this repository, then:

```bash
export OPENPOKER_KEY_FILE="$HOME/.config/triplo/openpoker.key"
target/release/triplo-turbo status            # read-only; confirms account and seating
target/release/triplo-turbo performance 1000 # read-only; latest account hands, all policies
target/release/triplo-turbo live 2000        # 100 BB, joins / recovers the public virtual-chip seat
```

Use a single live process per key. Open Poker replaces the existing connection when a second client opens the same seat. The live client enables the public league’s free virtual-chip auto-rebuy after queue entry. SIGINT or SIGTERM requests a table leave; the service reconnects after transport failures. Store the journal privately because decision traces include our own cards. API keys and turn tokens are not logged.

### Continuous operation on Linux

```bash
scripts/install-service live "$OPENPOKER_KEY_FILE"
systemctl --user status triplo-turbo-live
journalctl --user -u triplo-turbo-live -f -o cat
scripts/live-summary --since '24 hours ago'
scripts/live-summary --since '24 hours ago' --hand-ids > runs/turbo-hand-ids.txt
target/release/triplo-turbo performance 5000 runs/turbo-hand-ids.txt
systemctl --user disable --now triplo-turbo-live
```

The installer detects the existing `triplo-equity-live` service on the development host. To intentionally hand over the same seat, disable and stop that unit first (`systemctl --user disable --now triplo-equity-live`), then install Turbo live. To roll back, disable/stop Turbo live and `systemctl --user enable --now triplo-equity-live`. On this machine the user manager supports lingering across logout. Make `runs/` first if saving output there.

**Shadow mode** follows the owner’s existing redacted player wire JSONL and writes decisions to its own journal. It has no key, never connects to a table, and cannot earn or lose chips. It can coexist with the live incumbent. Use `replay` for historical data or `shadow` for new rows:

```bash
target/release/triplo-turbo replay /path/to/trace.jsonl
scripts/install-service shadow /path/to/trace.jsonl
journalctl --user -u triplo-turbo-shadow -n 5 -o cat
```

## Measuring strength

Primary metric: **100 × mean(net chips from one hand / that hand’s big blind)**, including folds and posted blinds. Neither rebuys nor account transfers count. Hands won %, maximum peak-to-trough loss in BB, rejection rate and p99 latency are supporting metrics. A 25% hands-won target is informative but can reward losing calls. `performance` reads settled hands from the bot-facing REST API, deduplicates IDs and checks recorded profit against both stack endpoints when available. It returns an approximate 95% interval over contiguous 100-hand blocks when there are at least ten blocks. Its optional list of IDs isolates a deployment and reports unmatched hands.

`arena BLOCKS SEATS SEED SAMPLES PROFILE FIELD` plays the candidate once per seat for each fresh deal; each hand starts at 100 BB. `scripted` uses simple calling/tight/pressure controls, `baseline` uses frozen equity-policy opponents, and `mixed` combines them. Uncertainty is computed over whole deal blocks, not the six correlated seat rotations. Local wins count a share of a contested pot (not an uncalled refund). Live metrics use the server’s winner list. Large gains against the scripted callers are a weak-opponent sanity check.

For a stronger test, `scripts/benchmark_reference.py` uses a separately trained checkpoint and independently implemented game engine in the owner's Triplo research checkout. Opponents use the checkpoint with its pressure guard, minimum 64 visits, and `equity-guarded-v3` fallback. It does not share persistent opponent statistics or hidden cards with the challenger. The simulator has half-BB chip units; Turbo can choose any legal raise, rounded to that unit, and the reference keeps its trained betting menu.

```bash
ref="$HOME/Triplo-Texas-Holdem-Poker-"
"$ref/.venv/bin/python" scripts/benchmark_reference.py "$ref" \
    --blocks 1000 --depths 50 100 200 --workers 6 --seed 40004931
```

## Training and architecture

- `src/cards.rs`: validated card ingress, bit masks, best-five evaluator. Tests cover the exhaustive 2,598,960 five-card class census and best-five-subset parity for seven cards.
- `src/arena.rs`: 2–6 seats, short all-ins and cumulative reopening, separate street/hand ledgers, dead money, uncalled returns, ties and side pots. The policy sees only its own hole cards and public information. Rayon distributes independent deal blocks; deck and policy RNG streams are independent.
- `src/policy.rs`: position and stack-aware preflop decisions, range-weighted Monte Carlo showdown shares, price/texture-dependent continuation and value sizing. Legacy profiles remain selectable in the arena as frozen controls. This is a heuristic and learned policy, not a solved equilibrium. With a reconciled live ledger the policy removes chips above hero's maximum contribution from the pot price; after incomplete recovery it falls back to the server pot. The equity approximation does not price every side-pot layer independently.
- `src/protocol.rs` and `src/live.rs`: server-owned actions and integer raise-to bounds, public/private state reduction, a bounded-time strategy worker, turn-bound responses and identical-payload reconnect retries. Replays are applied before the authoritative resync snapshot. Forward sequence gaps alone do not trigger recovery. Tests exercise a real loopback WebSocket, reconnect before acknowledgment, rejection, and stale-token suppression. See the [V2 wire contract](https://docs.openpoker.ai/api-reference/websocket-protocol/).
- `src/shadow.rs`: player-trace replay and read-only tailing; `policy-stdio` exposes an offline JSONL interface for independent engines. Supply a hero snapshot, preceding public events and a deterministic seed.

To reproduce the first training stage, run `target/release/triplo-turbo train-preflop 400000 452100701 > runs/preflop-fit.json`. It branches the same deal at a preflop decision, plays both actions to settlement against sampled opponents, and estimates their **difference in BB**, not how often they won. Only observable buckets with ≥100 paired samples and a conservative positive lower bound receive an override. `models/preflop.json` stores the selected bucket IDs; detailed training returns can stay off the live machine.

The second training stage, `scripts/train_response.py`, makes paired preflop and postflop decisions against the trained reference. It uses independent training seeds, records at most one observation per hand per bucket/action, and shrinks to the first-stage policy when evidence is weak. Profile 20 uses **at most one learned response per hand**, and limits the chips exposed by speculative responses. This matters: single-action rollout evidence does not justify repeatedly reraising on the same hand. After using a response it returns to profile 17 for that hand. The independent engine, local arena, live connector, and shadow runner enforce the same budget.

To reproduce response training: `"$ref/.venv/bin/python" scripts/train_response.py "$ref" --hands 90000 --workers 8 --seed 73900001 > runs/response-fit.json`. The cross-population screening experiment is `target/release/triplo-turbo train-robust 1000000 38811001 > runs/robust-fit.json`. Its restricted bucket list is retained as profile 19 for comparison. Training output must be rebuilt into a release; no running process changes strategy by itself.
