//! Triplo Turbo V2: Bayesian Range-vs-Range EV-Tree & HUD-Exploitative 6-Max Engine.
//! Uses ONLY player-visible game state and persistent public table HUD observations.

use crate::cards::Card;
use crate::eval::{HeroHandInfo, evaluate_against_ranges};
use crate::hud::OppHud;
use crate::policy::{Action, Chips, Decision, Legal};
use crate::range::{BoardFast, ComboRange, Pos, PreflopLine, allin_strength, playability_score};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

pub const V2_PROFILE: u8 = 40;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct V2Params {
    pub open_width_scale: f64,
    pub steal_aggression: f64,
    pub three_bet_scale: f64,
    pub bb_defend_scale: f64,
    pub fold_bias: f64,
    pub bluff_fold_discount: f64,
    pub value_future_bonus: f64,
    pub ip_eqr: f64,
    pub oop_eqr: f64,
    pub call_ip_eqr: f64,
    pub call_oop_eqr: f64,
    pub raise_respect_rio: f64,
    pub station_value_boost: f64,
    pub cbet_pfa_bonus: f64,
    pub draw_implied_scale: f64,
    pub close_ev_mix_frac: f64,
}

impl V2Params {
    pub fn embedded() -> &'static Self {
        static PARAMS: OnceLock<V2Params> = OnceLock::new();
        PARAMS.get_or_init(|| {
            serde_json::from_str(include_str!("../models/v2.json")).expect("valid models/v2.json")
        })
    }
}

#[derive(Clone, Debug)]
pub struct V2Opp {
    pub seat: usize,
    pub pos: Pos,
    pub stack: Chips,
    pub bet: Chips,
    pub preflop_line: PreflopLine,
    pub hud: OppHud,
    pub range: ComboRange,
    pub is_aggressor: bool,
    pub is_live: bool,
}

#[derive(Clone, Debug)]
pub struct V2Context {
    pub hole: [Card; 2],
    pub board: Vec<Card>,
    pub bb: Chips,
    pub pot: Chips,
    pub bet: Chips,
    pub stack: Chips,
    pub effective: Chips,
    pub legal: Legal,
    pub pos: Pos,
    pub in_position: bool,
    pub actors_behind: u8,
    pub preflop_raises: u8,
    pub limpers: u8,
    pub hero_was_pfa: bool,
    pub street_raises: u8,
    pub facing_check_raise: bool,
    pub opponents: Vec<V2Opp>,
}

pub fn decide_v2(ctx: &V2Context, seed: u64, samples: usize) -> Decision {
    decide_v2_with_params(ctx, seed, samples, V2Params::embedded())
}

pub fn decide_v2_with_params(
    ctx: &V2Context,
    seed: u64,
    samples: usize,
    params: &V2Params,
) -> Decision {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    if ctx.board.is_empty() {
        preflop_v2(ctx, params, &mut rng)
    } else {
        postflop_v2(ctx, params, samples, &mut rng)
    }
}

fn clamp_raise(legal: Legal, wanted: Chips) -> Option<Action> {
    if let Some((lo, hi)) = legal.raise {
        Some(Action::Raise(wanted.clamp(lo, hi)))
    } else if let Some(j) = legal.jam {
        if j <= wanted.saturating_mul(13) / 10 {
            Some(Action::AllIn)
        } else {
            None
        }
    } else {
        None
    }
}

fn preflop_v2(ctx: &V2Context, params: &V2Params, rng: &mut ChaCha8Rng) -> Decision {
    let l = ctx.legal;
    let bb = ctx.bb.max(1) as f64;
    let call = l.call.unwrap_or(0).min(ctx.stack);
    let odds = call as f64 / (ctx.pot as f64 + call as f64).max(1.0);
    let cost_bb = call as f64 / bb;
    let depth_bb = ctx.effective as f64 / bb;
    let passive = l.passive().expect("nonempty legal menu");
    let cont = if l.check {
        Action::Check
    } else if l.call.is_some() {
        Action::Call
    } else {
        passive
    };

    let c0 = ctx.hole[0];
    let c1 = ctx.hole[1];
    let hi = c0.rank().max(c1.rank());
    let lo = c0.rank().min(c1.rank());
    let pair = hi == lo;
    let suited = c0.suit() == c1.suit();
    let gap = hi - lo;
    let score = playability_score(c0, c1);
    let allin = allin_strength(c0, c1);

    let is_aa_kk = pair && hi >= 13;
    let is_premium = is_aa_kk || (pair && hi == 12) || (hi == 14 && lo == 13);
    let is_strong = is_premium
        || (pair && hi >= 10)
        || (hi == 14 && lo == 12)
        || (suited && hi == 14 && lo >= 11)
        || (suited && hi == 13 && lo == 12);
    let is_suited_wheel = suited && hi == 14 && lo <= 5;
    let is_suited_conn = suited && gap == 1 && lo >= 4;
    let is_suited_gap = suited && gap == 2 && lo >= 5;

    let n_opp = ctx.opponents.len().max(1);
    let avg_fold_open = ctx.opponents.iter().map(|o| o.hud.fold_to_open).sum::<f64>() / n_opp as f64;
    let avg_3bet = ctx.opponents.iter().map(|o| o.hud.three_bet).sum::<f64>() / n_opp as f64;
    let avg_sticky = ctx.opponents.iter().map(|o| o.hud.stickiness()).sum::<f64>() / n_opp as f64;

    // Identify primary raiser if facing a raise
    let raiser = ctx
        .opponents
        .iter()
        .find(|o| o.is_aggressor)
        .or_else(|| {
            ctx.opponents
                .iter()
                .find(|o| matches!(o.preflop_line, PreflopLine::OpenRaise | PreflopLine::ThreeBet | PreflopLine::FourBetPlus))
        });

    let mut action = passive;
    let mut reason: &'static str = "v2_pf_fold";

    // Short-stack push/fold mode (<= 14 BB effective)
    if depth_bb <= 14.0 {
        let target_bb = (ctx.bet + call) as f64 / bb;
        if ctx.preflop_raises == 0 && target_bb <= 1.0 {
            let push_thresh = match ctx.pos {
                Pos::Btn | Pos::Sb => 0.44,
                Pos::Co => 0.54,
                Pos::Hj => 0.63,
                _ => 0.70,
            } - 0.08 * (14.0 - depth_bb) / 10.0;
            if allin >= push_thresh || pair || (hi == 14 && suited) {
                if let Some(a) = l.jam.map(|_| Action::AllIn).or_else(|| clamp_raise(l, ctx.bet + ctx.stack)) {
                    return Decision { action: a, reason: "v2_pf_short_jam", equity: None, pot_odds: odds, samples: 0 };
                }
            }
        } else {
            let call_thresh = if depth_bb <= 8.0 {
                0.56 + 0.25 * odds
            } else {
                0.66 + 0.28 * odds
            };
            if is_premium || allin >= call_thresh || (pair && hi >= 7 && odds <= 0.42) {
                if let Some(a) = l.jam.map(|_| Action::AllIn).or_else(|| l.call.map(|_| Action::Call)) {
                    return Decision { action: a, reason: "v2_pf_short_call_jam", equity: None, pot_odds: odds, samples: 0 };
                }
            }
        }
        return Decision {
            action: if l.check { Action::Check } else { passive },
            reason: "v2_pf_short_fold",
            equity: None,
            pot_odds: odds,
            samples: 0,
        };
    }

    // Standard / Deep-stack (15 BB to 250+ BB)
    let current_bet_bb = (ctx.bet + call) as f64 / bb;

    if ctx.preflop_raises == 0 && current_bet_bb <= 1.05 {
        if ctx.limpers == 0 {
            // === CASE 1: UNOPENED POT (RFI) ===
            // Base percentile threshold (higher score = stronger hand; 1.0 - width)
            let base_width = ctx.pos.rfi_width() * params.open_width_scale;
            // Adjust for players behind: steal wider vs folders, tighter vs 3-bettors/stations
            let fold_adj = (avg_fold_open - 0.62) * params.steal_aggression * 2.0;
            let tb_adj = -(avg_3bet - 0.04) * 1.4;
            let width = (base_width + fold_adj + tb_adj).clamp(0.11, 0.72);
            // Convert width [0.11..0.72] to playability score threshold roughly matching 169 distribution
            let thresh = score_cutoff_for_width(width);

            // Against very sticky tables, prefer high cards / pairs over tiny offsuit steals
            let sticky_pen = if avg_sticky > 0.60 && !pair && !suited && hi < 12 {
                0.06
            } else {
                0.0
            };

            if score >= thresh + sticky_pen {
                // Uniform position-based sizing (no telegraphing hand strength!)
                // Size up uniformly vs sticky tables to extract maximum value!
                let sticky_bump = if avg_sticky > 0.58 { 0.5 } else { 0.0 };
                let size_bb = match ctx.pos {
                    Pos::Sb => 3.0 + sticky_bump,
                    Pos::Btn => 2.5 + sticky_bump,
                    _ => 2.5 + sticky_bump,
                };
                if let Some(a) = clamp_raise(l, (size_bb * bb) as Chips) {
                    action = a;
                    reason = "v2_pf_rfi";
                }
            } else if ctx.pos == Pos::Sb && cost_bb <= 0.6 && score >= 0.42 && avg_fold_open < 0.55 {
                // Cheap SB complete in 0%-rake game against sticky BB
                if l.call.is_some() {
                    action = Action::Call;
                    reason = "v2_pf_sb_complete";
                }
            } else if l.check {
                action = Action::Check;
                reason = "v2_pf_bb_check";
            }
        } else {
            // === CASE 2: FACING LIMPERS (ISO-RAISE OR OVER-LIMP) ===
            let iso_width = if ctx.pos.is_late() { 0.28 } else { 0.19 } * params.open_width_scale;
            let iso_thresh = score_cutoff_for_width(iso_width);
            if score >= iso_thresh || (pair && hi >= 7) {
                let iso_bb = 3.2 + ctx.limpers as f64 * 1.0 + if ctx.pos.is_blind() { 0.8 } else { 0.0 };
                if let Some(a) = clamp_raise(l, (iso_bb * bb) as Chips) {
                    action = a;
                    reason = "v2_pf_iso";
                }
            } else if l.check {
                action = Action::Check;
                reason = "v2_pf_bb_check";
            } else if l.call.is_some()
                && cost_bb <= 1.05
                && depth_bb >= 25.0
                && (pair || suited && (hi >= 11 || gap <= 2) || (hi == 14 && lo >= 9))
            {
                // 0% rake over-limp with high-implied-odds multiway hands
                action = Action::Call;
                reason = "v2_pf_overlimp";
            }
        }
    } else if ctx.preflop_raises == 1 {
        // === CASE 3: FACING AN OPEN RAISE (3-BET / CALL / FOLD) ===
        let r_hud = raiser.map(|o| o.hud).unwrap_or_else(OppHud::pool_prior);
        let r_pos = raiser.map(|o| o.pos).unwrap_or(Pos::Hj);
        let r_maniac = r_hud.pfr >= 0.34 || (r_hud.vpip >= 0.55 && r_hud.pfr >= 0.26);
        let r_loose = r_maniac || r_hud.pfr > 0.21 || matches!(r_pos, Pos::Co | Pos::Btn | Pos::Sb);
        let r_tight = !r_maniac && r_hud.pfr < 0.13 && matches!(r_pos, Pos::Utg | Pos::Hj);
        let r_station = r_hud.fold_to_3bet < 0.36 || r_hud.stickiness() > 0.62;
        let cold_callers = ctx
            .opponents
            .iter()
            .filter(|o| o.preflop_line == PreflopLine::ColdCall)
            .count() as f64;

        // 3-bet value range (linearly expands against loose/maniac/late openers -> ~6-10% total 3-bet!)
        let value_3bet = is_premium
            || (pair && hi >= 10 && (!r_tight || cost_bb <= 4.0))
            || (suited && hi == 14 && lo >= 11)
            || (hi == 14 && lo == 12 && (r_loose || ctx.pos.is_late() || ctx.pos.is_blind()))
            || (suited && hi == 13 && lo == 12 && !r_tight)
            || (r_loose
                && (pair && hi >= 9
                    || (hi == 14 && lo >= 11)
                    || (suited && hi == 14 && lo >= 10)
                    || (suited && hi == 13 && lo >= 11)
                    || (suited && hi == 12 && lo == 11)))
            || (r_maniac
                && cost_bb <= 7.0
                && (pair && hi >= 7
                    || (hi == 14 && lo >= 9)
                    || (hi == 13 && lo >= 11)
                    || (suited && hi >= 12 && lo >= 10)))
            || (r_station
                && cost_bb <= 4.5
                && (pair && hi >= 9
                    || (hi == 14 && lo >= 11)
                    || (suited && hi == 13 && lo >= 10)));

        // Polarized 3-bet bluff (suited wheel aces, suited connectors, suited broadways IP/blinds)
        let bluff_3bet = !r_station
            && r_hud.fold_to_3bet >= 0.42
            && depth_bb >= 35.0
            && cost_bb <= 4.5
            && cold_callers <= 1.0
            && (ctx.pos.is_late() || ctx.pos == Pos::Sb || ctx.pos == Pos::Bb)
            && (is_suited_wheel
                || (is_suited_conn && lo >= 5 && (ctx.pos.is_late() || r_loose))
                || (suited && hi == 13 && lo >= 9 && r_loose)
                || (suited && hi == 12 && lo == 10 && r_loose))
            && rng.gen_bool((0.55 * params.three_bet_scale).clamp(0.15, 0.90));

        if (value_3bet || bluff_3bet) && cost_bb <= 10.0 {
            let mult = if ctx.pos.is_late() { 3.0 } else { 3.5 } + 0.75 * cold_callers;
            let to = ((ctx.bet + call) as f64 * mult) as Chips;
            let to = if depth_bb <= 26.0 && value_3bet {
                ctx.bet.saturating_add(ctx.effective)
            } else {
                to
            };
            if let Some(a) = clamp_raise(l, to) {
                action = a;
                reason = if value_3bet { "v2_pf_3bet_val" } else { "v2_pf_3bet_bluff" };
            }
        } else if l.call.is_some() {
            // Disciplined 0%-rake calling / BB defense logic (adapts to maniac openers!)
            let maniac_price_bonus = if r_maniac { 2.5 } else { 0.0 };
            let mut defend = false;
            if is_premium && cost_bb <= 20.0 {
                defend = true;
            } else if ctx.pos == Pos::Bb {
                if cost_bb <= 3.8 + maniac_price_bonus {
                    let base_w = if r_maniac {
                        0.62
                    } else if r_loose {
                        0.56
                    } else if r_tight {
                        0.34
                    } else {
                        0.45
                    };
                    let price_adj = (0.30 - odds) * 1.1;
                    let w = ((base_w + price_adj) * params.bb_defend_scale).clamp(0.18, 0.74);
                    defend = score >= score_cutoff_for_width(w)
                        || (pair && depth_bb >= cost_bb * 9.5)
                        || (suited && (hi == 14 || gap <= 2) && odds <= 0.37);
                } else if cost_bb <= 7.0 + maniac_price_bonus {
                    defend = is_strong
                        || (r_maniac && (pair || hi == 14 && lo >= 8 || suited && hi >= 11))
                        || (pair && depth_bb >= cost_bb * 11.5 && odds <= 0.36)
                        || (suited && hi >= 12 && lo >= 9 && odds <= 0.36)
                        || (is_suited_conn && lo >= 7 && depth_bb >= cost_bb * 14.0 && odds <= 0.33);
                } else if cost_bb <= 14.0 {
                    defend = (is_strong || r_maniac && (pair && hi >= 7 || hi == 14 && lo >= 10))
                        && odds <= 0.40;
                }
            } else if ctx.pos.is_late() {
                if cost_bb <= 3.8 + maniac_price_bonus {
                    let base_w = if r_maniac {
                        0.38
                    } else if r_loose {
                        0.30
                    } else if r_tight {
                        0.18
                    } else {
                        0.24
                    };
                    defend = score >= score_cutoff_for_width(base_w)
                        || (pair && depth_bb >= cost_bb * 10.5)
                        || (is_suited_conn && lo >= 5 && depth_bb >= cost_bb * 13.0)
                        || (is_suited_gap && lo >= 7 && depth_bb >= cost_bb * 15.0 && cold_callers >= 1.0)
                        || (suited && hi == 14 && lo >= 5);
                } else if cost_bb <= 6.5 + maniac_price_bonus {
                    defend = is_strong
                        || (r_maniac && (pair && hi >= 6 || hi == 14 && lo >= 10 || suited && hi >= 12))
                        || (pair && depth_bb >= cost_bb * 12.0)
                        || (suited && hi >= 13 && lo >= 10);
                }
            } else {
                if cost_bb <= 3.5 + maniac_price_bonus {
                    defend = is_strong
                        || (r_maniac && (pair && hi >= 6 || hi == 14 && lo >= 10))
                        || (pair && depth_bb >= cost_bb * 12.0)
                        || (suited && hi == 14 && lo >= 10)
                        || (suited && hi >= 12 && lo >= 10)
                        || (cold_callers >= 1.0 && (is_suited_conn && lo >= 6) && depth_bb >= 40.0);
                } else if cost_bb <= 6.0 + maniac_price_bonus {
                    defend = is_strong || (pair && hi >= 8 && depth_bb >= cost_bb * 12.0);
                }
            }

            if defend {
                action = cont;
                reason = "v2_pf_defend_call";
            }
        }
    } else if ctx.preflop_raises == 2 {
        // === CASE 4: FACING A 3-BET ===
        let r_hud = raiser.map(|o| o.hud).unwrap_or_else(OppHud::pool_prior);
        let maniac_3b = r_hud.three_bet >= 0.13 || (r_hud.pfr >= 0.45 && r_hud.three_bet >= 0.08);
        let loose_3b = maniac_3b || r_hud.three_bet >= 0.065 || r_hud.aggression() > 0.60;
        let nit_3b = !loose_3b && r_hud.three_bet < 0.028 && r_hud.confidence > 0.35;

        let value_4bet = is_aa_kk
            || (pair && hi == 12 && (!nit_3b || depth_bb <= 95.0))
            || (hi == 14 && lo == 13 && (suited || (!nit_3b && depth_bb <= 160.0) || depth_bb <= 95.0))
            || (loose_3b && depth_bb <= 140.0 && (pair && hi >= 11 || (hi == 14 && lo == 12 && suited)))
            || (maniac_3b && depth_bb <= 130.0 && (pair && hi >= 9 || (hi == 14 && lo >= 11) || (suited && hi == 13 && lo == 12)));

        let bluff_4bet = loose_3b
            && r_hud.three_bet >= 0.09
            && depth_bb >= 80.0
            && cost_bb <= 14.0
            && is_suited_wheel
            && rng.gen_bool(0.32);

        if value_4bet || bluff_4bet {
            let to = if depth_bb <= 45.0 || cost_bb >= depth_bb * 0.28 {
                ctx.bet.saturating_add(ctx.effective)
            } else {
                ((ctx.bet + call) as f64 * 2.35) as Chips
            };
            if let Some(a) = clamp_raise(l, to) {
                action = a;
                reason = if value_4bet { "v2_pf_4bet_val" } else { "v2_pf_4bet_bluff" };
            } else if l.call.is_some() && value_4bet {
                action = Action::Call;
                reason = "v2_pf_call_3bet_val";
            }
        } else if l.call.is_some() {
            // Disciplined 3-bet calling range (widens cleanly vs hyper-3-betting maniacs!)
            let max_c = if maniac_3b { 14.5 } else { 10.5 };
            let call_3b = is_premium
                || (pair && hi >= 10 && cost_bb <= max_c + 3.0 && odds <= 0.43)
                || (pair && hi >= 6 && cost_bb <= max_c && depth_bb >= cost_bb * 10.5 && odds <= 0.38)
                || (pair && hi >= 2 && cost_bb <= 8.0 && depth_bb >= cost_bb * 13.5 && odds <= 0.33)
                || (suited && hi == 14 && lo >= 10 && cost_bb <= max_c && odds <= 0.39)
                || (suited && hi == 13 && lo >= 11 && cost_bb <= max_c && odds <= 0.37)
                || (hi == 14 && lo == 12 && cost_bb <= max_c && (!nit_3b || ctx.in_position || maniac_3b) && odds <= 0.38)
                || (maniac_3b && cost_bb <= 13.0 && odds <= 0.40 && (hi == 14 && lo >= 9 || hi == 13 && lo >= 11 || suited && hi >= 11 && lo >= 9))
                || (ctx.in_position && cost_bb <= 9.0 && depth_bb >= 55.0 && odds <= 0.35
                    && (suited && hi >= 11 && lo >= 9 || is_suited_conn && lo >= 7));
            if call_3b {
                action = Action::Call;
                reason = "v2_pf_call_3bet";
            }
        }
    } else {
        // === CASE 5: FACING 4-BET+ OR HUGE JAM ===
        let r_hud = raiser.map(|o| o.hud).unwrap_or_else(OppHud::pool_prior);
        let maniac = r_hud.three_bet >= 0.11 || r_hud.aggression() > 0.68 || r_hud.pfr >= 0.45;

        let stack_off = is_aa_kk
            || (pair && hi == 12 && (depth_bb <= 120.0 || maniac || odds <= 0.41))
            || (hi == 14 && lo == 13 && suited && (depth_bb <= 120.0 || maniac || odds <= 0.41))
            || (hi == 14 && lo == 13 && (depth_bb <= 75.0 || maniac || odds <= 0.39))
            || (pair && hi == 11 && (depth_bb <= 55.0 || maniac || odds <= 0.36))
            || (pair && hi == 10 && (depth_bb <= 35.0 || maniac && odds <= 0.40))
            || (maniac && pair && hi >= 9 && depth_bb <= 60.0)
            || (odds <= 0.25 && (is_strong || pair && hi >= 7));

        if stack_off {
            if is_aa_kk && let Some(a) = l.jam.map(|_| Action::AllIn).or_else(|| clamp_raise(l, ctx.bet + ctx.effective)) {
                action = a;
                reason = "v2_pf_5bet_jam";
            } else if l.call.is_some() {
                action = Action::Call;
                reason = "v2_pf_call_4bet";
            } else if let Some(a) = l.jam.map(|_| Action::AllIn) {
                action = a;
                reason = "v2_pf_5bet_jam";
            }
        }
    }

    if !l.permits(action) {
        action = passive;
        reason = "v2_pf_fallback";
    }
    Decision {
        action,
        reason,
        equity: None,
        pot_odds: odds,
        samples: 0,
    }
}

/// Maps target top-fraction width `w` in [0.05..0.75] to playability_score cutoff.
fn score_cutoff_for_width(w: f64) -> f64 {
    let t = crate::range::combo_tables();
    let target_pct = (1.0 - w).clamp(0.05, 0.99) as f32;
    // Find approximate playability score at target_pct (fast linear interpolation over key anchors)
    // Exact lookup: scan 1326 once? Even simpler: binary-friendly precomputed 101-point table!
    static CUTOFFS: OnceLock<[f64; 101]> = OnceLock::new();
    let cutoffs = CUTOFFS.get_or_init(|| {
        let mut vals: Vec<f32> = t.playability.to_vec();
        vals.sort_by(|a, b| a.total_cmp(b));
        let mut arr = [0.0f64; 101];
        for i in 0..=100 {
            let idx = (i * 1325) / 100;
            arr[i] = vals[idx] as f64;
        }
        arr
    });
    let idx = ((target_pct * 100.0).round() as usize).min(100);
    cutoffs[idx]
}

fn postflop_v2(
    ctx: &V2Context,
    params: &V2Params,
    samples: usize,
    rng: &mut ChaCha8Rng,
) -> Decision {
    let l = ctx.legal;
    let pot = ctx.pot.max(1) as f64;
    let call = l.call.unwrap_or(0).min(ctx.stack);
    let call_f = call as f64;
    let odds = call_f / (pot + call_f).max(1.0);
    let eff = ctx.effective.max(1) as f64;
    let spr = eff / pot;
    let street = match ctx.board.len() {
        3 => 1,
        4 => 2,
        _ => 3,
    };
    let is_river = street == 3;
    let passive = l.passive().expect("nonempty legal menu");
    let cont = if l.check {
        Action::Check
    } else if l.call.is_some() {
        Action::Call
    } else {
        passive
    };

    let board_info = BoardFast::analyze(&ctx.board);
    let hero_info = HeroHandInfo::analyze(ctx.hole, &ctx.board, &board_info);

    // Guaranteed board split: never fold, never pointlessly raise
    if hero_info.is_nuts && hero_info.board_plays {
        return Decision {
            action: cont,
            reason: "v2_locked_split",
            equity: Some(0.5),
            pot_odds: odds,
            samples: 0,
        };
    }

    let opp_pairs: Vec<(ComboRange, OppHud)> = ctx
        .opponents
        .iter()
        .map(|o| (o.range.clone(), o.hud))
        .collect();

    if opp_pairs.is_empty() {
        return Decision {
            action: cont,
            reason: "v2_no_opps",
            equity: Some(1.0),
            pot_odds: odds,
            samples: 0,
        };
    }

    let mc_samples = if is_river { samples.min(384) } else { samples.clamp(256, 512) };
    let (mw_eq, opp_evals) = evaluate_against_ranges(
        ctx.hole,
        &ctx.board,
        &board_info,
        &opp_pairs,
        mc_samples,
        rng,
    );

    let n_opp = opp_evals.len();
    let draw_outs = hero_info.draw_strength();
    let max_aggr = opp_evals.iter().map(|o| o.hud.aggression()).fold(0.0f64, f64::max);
    let avg_sticky = opp_evals.iter().map(|o| o.hud.stickiness()).sum::<f64>() / n_opp as f64;

    // Identify the bettor/raiser HUD if facing a bet
    let aggr_hud = ctx
        .opponents
        .iter()
        .find(|o| o.is_aggressor)
        .map(|o| o.hud)
        .unwrap_or(opp_evals[0].hud);

    // === 1. PASSIVE ACTION EV (FOLD / CHECK / CALL) ===
    let mut best_actions: Vec<(Action, f64, &'static str)> = Vec::with_capacity(10);

    if call > 0 {
        if l.fold {
            best_actions.push((Action::Fold, 0.0, "v2_fold"));
        }
        if l.call.is_some() {
            // Compute realized equity when calling
            let base_eqr = if is_river {
                1.0
            } else if ctx.in_position {
                params.call_ip_eqr
            } else {
                params.call_oop_eqr
            };
            // Strong hands & draws realize more; weak/dominated hands OOP realize less
            let hand_eqr = if is_river {
                1.0
            } else if hero_info.bucket >= 7 {
                1.08
            } else if draw_outs >= 7.5 {
                1.02
            } else if hero_info.bucket >= 5 {
                0.98
            } else {
                0.90
            };
            let eq_realized = (mw_eq * base_eqr * hand_eqr).clamp(0.0, 1.0);

            // Implied odds on flop/turn for draws and hidden monsters
            let implied = if !is_river && spr > 1.2 {
                let draw_factor = (draw_outs / 9.0).min(1.5);
                let stick_mult = 0.7 + 0.9 * avg_sticky;
                let rem_eff = (eff - call_f).max(0.0).min(pot * 2.5);
                if draw_outs >= 7.0 {
                    rem_eff * params.draw_implied_scale * draw_factor * stick_mult * if street == 1 { 1.0 } else { 0.6 }
                } else if hero_info.bucket >= 7 {
                    rem_eff * 0.10 * stick_mult
                } else {
                    0.0
                }
            } else {
                0.0
            };

            // Reverse implied odds & raise-respect penalty:
            // On OpenPoker, postflop raises / huge river bets from non-maniacs are heavily value-weighted!
            let mut rio = 0.0;
            if ctx.street_raises >= 2 && hero_info.bucket <= 6 && !hero_info.is_nuts {
                let passive_factor = (1.0 - aggr_hud.aggression()).clamp(0.15, 0.95);
                let one_pair_vuln = if hero_info.bucket <= 4 { 1.35 } else if hero_info.bucket == 5 { 1.0 } else { 0.65 };
                rio += call_f * params.raise_respect_rio * passive_factor * one_pair_vuln * if is_river { 1.35 } else { 1.0 };
            }
            if is_river && call_f > pot * 0.85 && hero_info.bucket <= 5 && aggr_hud.river_agg < 0.32 {
                rio += call_f * 0.10;
            }
            if hero_info.board_plays && !hero_info.is_nuts {
                // Calling with board-only pair/high card is almost always losing or chopping
                rio += call_f * 0.20;
            }

            let ev_call = if hero_info.is_nuts {
                (pot + call_f) * 0.99 - call_f
            } else {
                (pot + call_f) * eq_realized - call_f + implied - rio
            };
            best_actions.push((Action::Call, ev_call, "v2_ev_call"));
        }
    } else if l.check {
        let base_eqr = if is_river {
            1.0
        } else if ctx.in_position {
            params.ip_eqr
        } else {
            params.oop_eqr
        };
        let mw_pen = if is_river { 1.0 } else { 0.96f64.powi((n_opp as i32 - 1).max(0)) };
        let hand_eqr = if is_river {
            1.0
        } else if hero_info.bucket >= 7 {
            1.06
        } else if hero_info.bucket >= 5 {
            1.0
        } else if draw_outs >= 7.0 {
            0.98
        } else {
            0.84
        };
        // Trap bonus: slowplay monsters ONLY on dry boards vs hyper-aggressive maniacs (NEVER vs stations!)
        let trap_bonus = if !is_river
            && hero_info.bucket >= 8
            && board_info.wetness < 0.30
            && max_aggr > 0.58
            && avg_sticky < 0.58
            && n_opp == 1
        {
            pot * 0.14 * (max_aggr - 0.45)
        } else {
            0.0
        };
        // Gentle check-to-PFA bonus ONLY for medium OOP hands vs aggressive high-cbet regs
        let pfa_high_cbet = ctx
            .opponents
            .iter()
            .any(|o| o.hud.cbet_flop > 0.56 && o.hud.stickiness() < 0.55);
        let check_to_pfa_bonus = if street == 1
            && !ctx.hero_was_pfa
            && !ctx.in_position
            && ctx.preflop_raises >= 1
            && hero_info.bucket <= 5
            && pfa_high_cbet
        {
            pot * 0.06
        } else {
            0.0
        };
        let ev_check =
            pot * (mw_eq * base_eqr * hand_eqr * mw_pen).clamp(0.0, 1.0) + trap_bonus + check_to_pfa_bonus;
        best_actions.push((Action::Check, ev_check, "v2_ev_check"));
    }

    // === 2. AGGRESSIVE CANDIDATE ACTIONS (BET / RAISE / ALL-IN) ===
    // Only consider betting/raising if legal and opponents have chips to win/fold
    let has_live_opp = ctx.opponents.iter().any(|o| o.is_live);
    if has_live_opp && (l.raise.is_some() || l.jam.is_some()) {
        let mut candidates: Vec<(Action, Chips, &'static str)> = Vec::with_capacity(6);
        let mut push_size = |target: Chips, tag: &'static str| {
            let act = if let Some((lo, hi)) = l.raise {
                let c = target.clamp(lo, hi);
                Action::Raise(c)
            } else if l.jam.is_some() {
                Action::AllIn
            } else {
                return;
            };
            let actual_to = match act {
                Action::Raise(x) => x,
                Action::AllIn => l.jam.unwrap_or(ctx.bet + ctx.stack),
                _ => return,
            };
            // Deduplicate very close sizings (within 8%)
            if !candidates.iter().any(|&(_, prev, _)| {
                let diff = actual_to.abs_diff(prev);
                diff * 12 <= actual_to.max(1)
            }) {
                candidates.push((act, actual_to, tag));
            }
        };

        let any_station = ctx
            .opponents
            .iter()
            .any(|o| o.hud.fold_to_bet < 0.30 && o.hud.confidence > 0.30);
        if call == 0 {
            let streets_left = (4 - street) as f64;
            let geo_frac = (((2.0 * spr + 1.0).powf(1.0 / streets_left) - 1.0) * 0.5).clamp(0.45, 1.55);
            if street == 1 {
                for &(frac, tag) in &[
                    (0.33, "v2_bet_small"),
                    (0.55, "v2_bet_med"),
                    (0.80, "v2_bet_large"),
                ] {
                    let add = (frac * pot).round() as Chips;
                    if add > 0 && (add as f64) <= eff * 0.92 {
                        push_size(ctx.bet.saturating_add(add), tag);
                    }
                }
                if (any_station && hero_info.bucket >= 6) || hero_info.bucket >= 8 {
                    for &(frac, tag) in &[(1.25, "v2_bet_over"), (1.75, "v2_bet_bomb")] {
                        let add = (frac * pot).round() as Chips;
                        if add > 0 && (add as f64) <= eff * 0.92 {
                            push_size(ctx.bet.saturating_add(add), tag);
                        }
                    }
                }
            } else if street == 2 {
                for &(frac, tag) in &[
                    (0.38, "v2_bet_small"),
                    (0.62, "v2_bet_med"),
                    (0.90, "v2_bet_large"),
                    (geo_frac, "v2_bet_geo"),
                    (1.30, "v2_bet_over"),
                ] {
                    let add = (frac * pot).round() as Chips;
                    if add > 0 && (add as f64) <= eff * 0.92 {
                        push_size(ctx.bet.saturating_add(add), tag);
                    }
                }
                if any_station && hero_info.bucket >= 6 {
                    let add = (1.85 * pot).round() as Chips;
                    if add > 0 && (add as f64) <= eff * 0.92 {
                        push_size(ctx.bet.saturating_add(add), "v2_bet_bomb");
                    }
                }
            } else {
                for &(frac, tag) in &[
                    (0.33, "v2_bet_small"),
                    (0.60, "v2_bet_med"),
                    (0.90, "v2_bet_large"),
                    (1.35, "v2_bet_over"),
                    (1.85, "v2_bet_bomb"),
                ] {
                    let add = (frac * pot).round() as Chips;
                    if add > 0 && (add as f64) <= eff * 0.92 {
                        push_size(ctx.bet.saturating_add(add), tag);
                    }
                }
            }
            if spr <= 1.9
                || (is_river && hero_info.is_nuts && spr <= 4.5)
                || (any_station && hero_info.bucket >= 8 && spr <= 3.5)
            {
                if let Some(j) = l.jam {
                    push_size(j, "v2_bet_jam");
                } else if let Some((_, hi)) = l.raise {
                    push_size(hi, "v2_bet_jam");
                }
            }
        } else if call > 0 {
            // Disciplined raising eligibility:
            // - Facing a raise/3-bet (street_raises >= 2): ONLY re-raise with genuine monsters!
            // - Facing a bet (street_raises == 1): raise with two-pair+, strong HU overpair/TPGK at low SPR,
            //   or strong HU semi-bluff draws (nut FD / 11+ out combo draw)!
            let allow_raise = if ctx.street_raises >= 2 {
                hero_info.is_nuts
                    || hero_info.bucket >= 8
                    || (hero_info.bucket == 7 && spr <= 3.5 && board_info.max_suit_count < 4)
            } else {
                hero_info.is_nuts
                    || hero_info.bucket >= 7
                    || (hero_info.bucket == 6
                        && n_opp == 1
                        && spr <= 5.0
                        && !board_info.has_trips
                        && board_info.max_suit_count < 4)
                    || (!is_river
                        && n_opp == 1
                        && (hero_info.nut_flush_draw || draw_outs >= 11.0))
                    || (is_river
                        && n_opp == 1
                        && hero_info.flush_blocker
                        && aggr_hud.fold_to_bet > 0.48
                        && spr <= 3.5)
            };
            if allow_raise {
                let pot_after_call = pot + call_f;
                for &(frac, tag) in &[(0.58, "v2_raise_std"), (0.95, "v2_raise_big")] {
                    let add = (call_f + frac * pot_after_call).round() as Chips;
                    if (add as f64) <= eff * 0.92 {
                        push_size(ctx.bet.saturating_add(add), tag);
                    }
                }
                if spr <= 2.0
                    || (is_river && hero_info.is_nuts && spr <= 4.5)
                    || (ctx.street_raises >= 2 && (hero_info.is_nuts || hero_info.bucket >= 8 && spr <= 3.5))
                {
                    if let Some(j) = l.jam {
                        push_size(j, "v2_raise_jam");
                    } else if let Some((_, hi)) = l.raise {
                        push_size(hi, "v2_raise_jam");
                    }
                }
            }
        }

        // PFA range-advantage & IP probe bonus on flop/turn c-bets!
        let pfa_fold_bonus = if call == 0 && n_opp <= 2 {
            if street == 1 && ctx.hero_was_pfa && board_info.wetness < 0.55 {
                params.cbet_pfa_bonus * if board_info.top_rank >= 11 { 1.25 } else { 0.85 }
            } else if ctx.in_position && street <= 2 && board_info.wetness < 0.45 {
                params.cbet_pfa_bonus * 0.55
            } else {
                0.0
            }
        } else {
            0.0
        };

        for (act, to_amt, tag) in candidates {
            if !l.permits(act) {
                continue;
            }
            let invest = to_amt.saturating_sub(ctx.bet) as f64;
            if invest <= call_f {
                continue;
            }
            let raise_over_call = (invest - call_f).max(1.0);
            let bet_to_pot = raise_over_call / (pot + call_f).max(1.0);
            let is_jam = matches!(act, Action::AllIn) || invest >= eff * 0.95;

            // Compute multi-opponent response probabilities and conditional equities
            let mut p_all_fold = 1.0f64;
            let mut p_no_raise = 1.0f64;
            let mut sum_call_prob = 0.0f64;
            let mut weighted_eq_call = 0.0f64;
            let mut sum_raise_prob = 0.0f64;
            let mut weighted_eq_raise = 0.0f64;

            for (idx, oe) in opp_evals.iter().enumerate() {
                let opp_commit = if ctx.opponents[idx].is_aggressor {
                    ctx.street_raises.max(1)
                } else {
                    0
                };
                let (pf, pc, pr, eq_c, eq_r) = oe.response_to(
                    bet_to_pot,
                    street,
                    opp_commit,
                    params.fold_bias + pfa_fold_bonus,
                );
                p_all_fold *= pf;
                p_no_raise *= 1.0 - pr;
                sum_call_prob += pc;
                weighted_eq_call += pc * eq_c;
                sum_raise_prob += pr;
                weighted_eq_raise += pr * eq_r;
            }

            let p_any_raise = if is_jam { 0.0 } else { (1.0 - p_no_raise).clamp(0.0, 0.95) };
            let p_called = (1.0 - p_all_fold - p_any_raise).max(0.0);

            let hu_eq_vs_call = if sum_call_prob > 1e-6 {
                (weighted_eq_call / sum_call_prob).clamp(0.0, 1.0)
            } else {
                mw_eq
            };
            // Multiway caller adjustment: if multiple opponents are likely to call, hero must beat ALL callers!
            let exp_callers = if p_called > 1e-4 {
                (sum_call_prob / p_called).clamp(1.0, n_opp as f64)
            } else {
                1.0
            };
            let eq_vs_call = if hero_info.is_nuts {
                0.995
            } else if n_opp == 1 {
                hu_eq_vs_call
            } else {
                hu_eq_vs_call.powf(1.0 + 0.72 * (exp_callers - 1.0))
            };

            let eq_vs_raise = if hero_info.is_nuts {
                0.99
            } else if sum_raise_prob > 1e-6 {
                (weighted_eq_raise / sum_raise_prob).clamp(0.0, 1.0)
            } else {
                0.15
            };

            // Guard against suicidal bluffs: discount fold equity slightly when bluffing with low equity
            let is_bluff = eq_vs_call < 0.34 && draw_outs < 7.0;
            let eff_p_fold = if is_bluff {
                // Extra penalty on overbet/raise bluffs with zero blockers or multiway
                let size_disc = if bet_to_pot > 1.0 { 0.86 } else { 1.0 };
                let blocker_boost = if is_river && hero_info.flush_blocker { 1.06 } else { 1.0 };
                let raise_bluff_disc = if call > 0 { 0.78 } else { 1.0 };
                (p_all_fold * params.bluff_fold_discount * size_disc * blocker_boost * raise_bluff_disc)
                    .clamp(0.0, 0.92)
            } else {
                p_all_fold
            };
            let eff_p_called = (1.0 - eff_p_fold - p_any_raise).max(0.0);

            // EV when called:
            // Opponent matches `raise_over_call` (plus any additional callers contribute `invest`)
            let opp_contrib = raise_over_call + (exp_callers - 1.0) * invest;
            let pot_when_called = pot + invest + opp_contrib;

            let bet_eqr = if is_river {
                1.0
            } else if ctx.in_position {
                1.02
            } else {
                0.94
            };
            let mut future_val = 0.0;
            if !is_river && spr > 1.0 {
                let rem = (eff - invest).max(0.0).min(pot_when_called * 1.6);
                if eq_vs_call > 0.62 {
                    // Building the pot with a strong value hand unlocks bigger turn/river value bets!
                    let station_extra = 1.0 + params.station_value_boost * (avg_sticky - 0.45).max(0.0) * 3.0;
                    future_val += rem * params.value_future_bonus * (eq_vs_call - 0.52) * station_extra;
                } else if draw_outs >= 7.5 {
                    // Semi-bluffing draws gains fold equity on next street + implied odds when hitting
                    future_val += rem * 0.06 * (draw_outs / 9.0);
                }
            }
            // Extra protection value on wet boards when holding strong one-pair/two-pair
            let protection = if !is_river
                && call == 0
                && hero_info.bucket >= 5
                && hero_info.bucket <= 7
                && board_info.wetness > 0.25
            {
                pot * 0.05 * board_info.wetness as f64
            } else {
                0.0
            };

            let ev_if_called =
                pot_when_called * (eq_vs_call * bet_eqr).clamp(0.0, 1.0) - invest + future_val + protection;

            // EV when raised: hero chooses best response (fold = -invest, or continue/jam)
            let ev_if_raised = if p_any_raise > 1e-5 {
                let opp_raise_add = (raise_over_call * 2.2).min((eff - invest).max(0.0));
                let hero_call_raise = opp_raise_add;
                let pot_after_r = pot + invest + raise_over_call + opp_raise_add + hero_call_raise;
                let ev_cont = pot_after_r * eq_vs_raise - (invest + hero_call_raise);
                let ev_fold_r = -invest;
                ev_fold_r.max(ev_cont)
            } else {
                0.0
            };

            let mut ev_bet =
                eff_p_fold * pot + eff_p_called * ev_if_called + p_any_raise * ev_if_raised;

            // Hard safety guardrail: never punt huge overbet/jam bluffs (>1.1x pot) with pure air on river
            // unless we actually have strong blockers & opponent folds frequently
            if is_bluff && is_river && bet_to_pot > 1.10 && (!hero_info.flush_blocker || avg_sticky > 0.52) {
                ev_bet -= invest * 0.25;
            }
            // Never raise-bluff into a 3-bet on the same street with pure air
            if is_bluff && ctx.street_raises >= 2 {
                ev_bet -= invest * 0.50;
            }
            // Respect check-raises: facing a check-raise is extra polar
            if ctx.facing_check_raise && hero_info.bucket <= 6 && !hero_info.is_nuts {
                ev_bet -= invest * 0.15;
            }
            // Slight preference against tiny nuisance raises when facing a bet with thin equity
            if call > 0 && hero_info.bucket <= 5 && draw_outs < 8.0 && eq_vs_call < 0.68 {
                ev_bet -= pot * 0.08;
            }

            best_actions.push((act, ev_bet, tag));
        }
    }

    // Select best EV action (with subtle mixing only when top actions are virtually tied!)
    best_actions.sort_by(|a, b| b.1.total_cmp(&a.1));
    let (mut chosen, best_ev, mut reason) = best_actions
        .first()
        .copied()
        .unwrap_or((passive, 0.0, "v2_fallback"));

    let mix_band = pot * params.close_ev_mix_frac;
    if mix_band > 0.0 && best_actions.len() >= 2 && (best_ev - best_actions[1].1) <= mix_band {
        // Softmax mix among top-2 nearly tied actions for unexploitability
        let diff = (best_ev - best_actions[1].1).max(0.0);
        let p_second = 0.5 * (-diff / (mix_band * 0.5).max(1e-6)).exp();
        if rng.gen_bool(p_second.clamp(0.0, 0.45)) {
            chosen = best_actions[1].0;
            reason = best_actions[1].2;
        }
    }

    if !l.permits(chosen) {
        chosen = passive;
        reason = "v2_legal_fallback";
    }

    Decision {
        action: chosen,
        reason,
        equity: Some(mw_eq),
        pot_odds: odds,
        samples: mc_samples,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::mask;

    #[test]
    fn v2_folds_42o_facing_open_and_bets_top_pair_value() {
        let c = |s| Card::parse(s).unwrap();
        let hud = OppHud::pool_prior();
        // Hand b658af70 regression: 4s 2h on BTN facing 2.5 BB open — MUST FOLD (profile 28 3-bet to 12 BB!)
        let hole = [c("4s"), c("2h")];
        let opp_r = ComboRange::from_preflop(Pos::Utg, PreflopLine::OpenRaise, &hud, mask(&hole));
        let ctx = V2Context {
            hole,
            board: vec![],
            bb: 20,
            pot: 80,
            bet: 0,
            stack: 1000,
            effective: 1000,
            legal: Legal {
                fold: true,
                check: false,
                call: Some(50),
                raise: Some((100, 1000)),
                jam: Some(1000),
            },
            pos: Pos::Btn,
            in_position: true,
            actors_behind: 2,
            preflop_raises: 1,
            limpers: 0,
            hero_was_pfa: false,
            street_raises: 1,
            facing_check_raise: false,
            opponents: vec![V2Opp {
                seat: 0,
                pos: Pos::Utg,
                stack: 950,
                bet: 50,
                preflop_line: PreflopLine::OpenRaise,
                hud,
                range: opp_r,
                is_aggressor: true,
                is_live: true,
            }],
        };
        let d = decide_v2(&ctx, 7, 256);
        assert_eq!(d.action, Action::Fold, "42o facing UTG open must fold, got {d:?}");

        // Hand d69f7660 regression: Qc 3d hitting top pair on Qs 7d 6s HU/3-way — MUST BET FOR VALUE!
        let hole2 = [c("Qc"), c("3d")];
        let board2 = vec![c("Qs"), c("7d"), c("6s")];
        let dead2 = mask(&hole2) | mask(&board2);
        let opp_r2 = ComboRange::from_preflop(Pos::Bb, PreflopLine::ColdCall, &hud, dead2);
        let ctx2 = V2Context {
            hole: hole2,
            board: board2,
            bb: 20,
            pot: 180,
            bet: 0,
            stack: 925,
            effective: 925,
            legal: Legal {
                fold: false,
                check: true,
                call: None,
                raise: Some((20, 925)),
                jam: Some(925),
            },
            pos: Pos::Co,
            in_position: true,
            actors_behind: 0,
            preflop_raises: 1,
            limpers: 0,
            hero_was_pfa: true,
            street_raises: 0,
            facing_check_raise: false,
            opponents: vec![V2Opp {
                seat: 2,
                pos: Pos::Bb,
                stack: 925,
                bet: 0,
                preflop_line: PreflopLine::ColdCall,
                hud,
                range: opp_r2,
                is_aggressor: false,
                is_live: true,
            }],
        };
        let d2 = decide_v2(&ctx2, 9, 384);
        assert!(
            matches!(d2.action, Action::Raise(_)),
            "Top pair Qs7d6s HU as PFA must value bet, got {d2:?}"
        );
    }

    #[test]
    fn v2_checks_22_on_a99_five_way() {
        let c = |s| Card::parse(s).unwrap();
        let hud = OppHud::pool_prior();
        let hole = [c("2d"), c("2c")];
        let board = vec![c("9h"), c("9s"), c("As")];
        let dead = mask(&hole) | mask(&board);
        let opps = (0..4)
            .map(|i| V2Opp {
                seat: i,
                pos: Pos::Hj,
                stack: 800,
                bet: 0,
                preflop_line: if i == 0 {
                    PreflopLine::OpenRaise
                } else {
                    PreflopLine::ColdCall
                },
                hud,
                range: ComboRange::from_preflop(
                    Pos::Hj,
                    if i == 0 {
                        PreflopLine::OpenRaise
                    } else {
                        PreflopLine::ColdCall
                    },
                    &hud,
                    dead,
                ),
                is_aggressor: i == 0,
                is_live: true,
            })
            .collect();
        let ctx = V2Context {
            hole,
            board,
            bb: 20,
            pot: 350,
            bet: 0,
            stack: 865,
            effective: 865,
            legal: Legal {
                fold: false,
                check: true,
                call: None,
                raise: Some((20, 865)),
                jam: Some(865),
            },
            pos: Pos::Sb,
            in_position: false,
            actors_behind: 4,
            preflop_raises: 1,
            limpers: 0,
            hero_was_pfa: false,
            street_raises: 0,
            facing_check_raise: false,
            opponents: opps,
        };
        let d = decide_v2(&ctx, 21, 256);
        assert_eq!(d.action, Action::Check, "22 on A99 5-way must check, got {d:?}");
    }
}
