//! Realistic 10-archetype 6-max OpenPoker benchmark, self-play, and parameter optimizer for V2.

use super::*;
use crate::cards::mask;
use crate::defense::Defense;
use crate::hud::{HudTracker, OppHud};
use crate::policy::{Action, Chips, Decision, View};
use crate::range::{BoardFast, ComboRange, Pos, PreflopLine, playability_score};
use crate::v2::{V2Context, V2Opp, V2Params, decide_v2_with_params};
use rand::Rng;
use serde_json::{Value, json};
use std::time::Instant;

#[derive(Clone, Copy, Debug)]
enum Archetype {
    GtoTag,
    LagShark,
    NitReg,
    CallingStation,
    LoosePassiveFish,
    Maniac,
    FitOrFold,
    PotOddsBot,
    Profile28,
    /// Calibrated to real OpenPoker regulars (Kairo/OMGtheNutsss/minhcasa/Kaiel/vivilma):
    /// ~27% VPIP, ~15% PFR, ~1% 3B, ~18% cbet, ~24% fold-to-bet, ~24% small-raise when strong!
    PassiveTrappy,
    /// Calibrated to real OpenPoker tight-foldy regs (Winters/ramses_888):
    /// ~21% VPIP, ~13% PFR, ~3.5% 3B, ~35% cbet, ~65% fold-to-bet, value-heavy barrels.
    TightFoldy,
    /// Calibrated to real OpenPoker hyper-LAGs (RoRo/Unkn0wn):
    /// ~52% VPIP, ~40% PFR, ~10% 3B, ~65% cbet, ~50% fold-to-bet, polarized big bets.
    HyperLag,
}

fn tag_params() -> V2Params {
    V2Params {
        open_width_scale: 0.90,
        steal_aggression: 0.04,
        three_bet_scale: 0.95,
        bb_defend_scale: 0.95,
        fold_bias: -0.04,
        bluff_fold_discount: 0.80,
        value_future_bonus: 0.10,
        ip_eqr: 1.03,
        oop_eqr: 0.85,
        call_ip_eqr: 0.98,
        call_oop_eqr: 0.87,
        raise_respect_rio: 0.16,
        station_value_boost: 0.04,
        cbet_pfa_bonus: 0.04,
        draw_implied_scale: 0.14,
        close_ev_mix_frac: 0.02,
    }
}

fn lag_params() -> V2Params {
    V2Params {
        open_width_scale: 1.24,
        steal_aggression: 0.16,
        three_bet_scale: 1.45,
        bb_defend_scale: 1.15,
        fold_bias: 0.04,
        bluff_fold_discount: 0.93,
        value_future_bonus: 0.14,
        ip_eqr: 1.06,
        oop_eqr: 0.88,
        call_ip_eqr: 1.01,
        call_oop_eqr: 0.90,
        raise_respect_rio: 0.10,
        station_value_boost: 0.12,
        cbet_pfa_bonus: 0.10,
        draw_implied_scale: 0.18,
        close_ev_mix_frac: 0.025,
    }
}

fn nit_params() -> V2Params {
    V2Params {
        open_width_scale: 0.62,
        steal_aggression: 0.0,
        three_bet_scale: 0.55,
        bb_defend_scale: 0.65,
        fold_bias: -0.14,
        bluff_fold_discount: 0.65,
        value_future_bonus: 0.08,
        ip_eqr: 1.00,
        oop_eqr: 0.82,
        call_ip_eqr: 0.94,
        call_oop_eqr: 0.82,
        raise_respect_rio: 0.25,
        station_value_boost: 0.02,
        cbet_pfa_bonus: 0.0,
        draw_implied_scale: 0.10,
        close_ev_mix_frac: 0.0,
    }
}

#[derive(Clone, Debug, Default)]
struct HandMeta {
    pf_raises: u8,
    limpers: u8,
    pfa_seat: Option<usize>,
    street_raises: u8,
    street_aggressor: Option<usize>,
    street_seen: usize,
    checked_street: [bool; 6],
    pf_line: [PreflopLine; 6],
    post_actions: [Vec<(usize, u8, f32, bool, bool)>; 6],
    post_barrels: [u8; 6],
    barreled_this_street: [bool; 6],
}

impl HandMeta {
    fn on_street(&mut self, street: usize) {
        if street != self.street_seen {
            self.street_seen = street;
            self.street_raises = 0;
            self.street_aggressor = None;
            self.checked_street = [false; 6];
            self.barreled_this_street = [false; 6];
        }
    }

    fn record_action(&mut self, seat: usize, street: usize, a: Action, v: &View, active_cnt: usize) {
        self.on_street(street);
        let call = v.legal.call.unwrap_or(0);
        let is_raise = matches!(a, Action::Raise(_))
            || (matches!(a, Action::AllIn) && v.stack > call);
        if street == 0 {
            if is_raise {
                self.pf_raises = self.pf_raises.saturating_add(1);
                self.pfa_seat = Some(seat);
                self.street_raises = self.street_raises.saturating_add(1);
                self.street_aggressor = Some(seat);
                self.pf_line[seat] = match self.pf_raises {
                    1 => PreflopLine::OpenRaise,
                    2 => PreflopLine::ThreeBet,
                    _ => PreflopLine::FourBetPlus,
                };
            } else if matches!(a, Action::Call | Action::AllIn) {
                if self.pf_raises == 0 {
                    self.limpers = self.limpers.saturating_add(1);
                    self.pf_line[seat] = PreflopLine::Limp;
                } else if self.pf_raises == 1 {
                    self.pf_line[seat] = PreflopLine::ColdCall;
                } else {
                    self.pf_line[seat] = PreflopLine::Call3Bet;
                }
            } else if matches!(a, Action::Check) {
                self.pf_line[seat] = PreflopLine::BbCheck;
            }
        } else {
            let pot_f = v.pot.max(1) as f32;
            let pot_before_bet_f = v.pot.saturating_sub(call).max(1) as f32;
            let is_mw = active_cnt >= 3;
            let is_donk = street == 1
                && self.street_raises == 0
                && self.pf_raises >= 1
                && self.pfa_seat.is_some_and(|p| p != seat);
            let (kind, ratio, donk_flag) = match a {
                Action::Check => {
                    self.checked_street[seat] = true;
                    (0u8, 0.0f32, false)
                }
                Action::Call => (1u8, call as f32 / pot_before_bet_f, false),
                Action::Raise(to) => {
                    let add = to.saturating_sub(v.bet);
                    let r = add as f32 / pot_f;
                    let k = if self.street_raises == 0 { 2u8 } else { 3u8 };
                    self.street_raises = self.street_raises.saturating_add(1);
                    self.street_aggressor = Some(seat);
                    if !self.barreled_this_street[seat] {
                        self.barreled_this_street[seat] = true;
                        self.post_barrels[seat] = self.post_barrels[seat].saturating_add(1);
                    }
                    (k, r, k == 2 && is_donk)
                }
                Action::AllIn => {
                    if is_raise {
                        let r = v.stack as f32 / pot_f;
                        let k = if self.street_raises == 0 { 2u8 } else { 3u8 };
                        self.street_raises = self.street_raises.saturating_add(1);
                        self.street_aggressor = Some(seat);
                        if !self.barreled_this_street[seat] {
                            self.barreled_this_street[seat] = true;
                            self.post_barrels[seat] = self.post_barrels[seat].saturating_add(1);
                        }
                        (k, r, k == 2 && is_donk)
                    } else {
                        (1u8, v.stack as f32 / pot_before_bet_f, false)
                    }
                }
                Action::Fold => return,
            };
            self.post_actions[seat].push((street, kind, ratio, is_mw, donk_flag));
        }
    }
}

fn build_v2_context(
    h: &Hand,
    seat: usize,
    meta: &HandMeta,
    hud: Option<&HudTracker>,
    names: &[&str; 6],
) -> V2Context {
    let v = h.view(seat);
    let pos_of = |s: usize| {
        let off = (s + h.n - h.button) % h.n;
        Pos::from_offset_from_btn(off, h.n)
    };
    let pos = pos_of(seat);

    // Determine postflop IP (acting last among remaining non-folded players with chips or all active)
    let postflop_order = |s: usize| (s + h.n - h.button - 1) % h.n;
    let hero_ord = postflop_order(seat);
    let actors_behind = h
        .active()
        .filter(|&j| j != seat && h.stack[j] > 0 && postflop_order(j) > hero_ord)
        .count() as u8;
    let in_position = actors_behind == 0;

    let dead = mask(&v.hole) | mask(&v.board);
    let mut opponents = Vec::with_capacity(5);
    let b3_info = (v.board.len() >= 3).then(|| BoardFast::analyze(&v.board[..3]));
    let b4_info = (v.board.len() >= 4).then(|| BoardFast::analyze(&v.board[..4]));
    let b5_info = (v.board.len() == 5).then(|| BoardFast::analyze(&v.board[..5]));

    for j in h.active().filter(|&j| j != seat) {
        let j_hud = hud
            .map(|t| t.get(names[j]))
            .unwrap_or_else(OppHud::pool_prior);
        let j_pos = pos_of(j);
        let mut range = ComboRange::from_preflop(j_pos, meta.pf_line[j], &j_hud, dead);
        if !v.board.is_empty() {
            for &(st, kind, ratio, is_mw, is_donk) in &meta.post_actions[j] {
                let (b_slice, bi) = match st {
                    1 if v.board.len() >= 3 => (&v.board[..3], b3_info.as_ref().unwrap()),
                    2 if v.board.len() >= 4 => (&v.board[..4], b4_info.as_ref().unwrap()),
                    3 if v.board.len() == 5 => (&v.board[..5], b5_info.as_ref().unwrap()),
                    _ => continue,
                };
                range.update_postflop(
                    b_slice,
                    mask(b_slice),
                    bi,
                    kind,
                    ratio,
                    st,
                    &j_hud,
                    is_mw,
                    is_donk,
                );
            }
        }
        opponents.push(V2Opp {
            seat: j,
            pos: j_pos,
            stack: h.stack[j],
            bet: h.bet[j],
            preflop_line: meta.pf_line[j],
            hud: j_hud,
            range,
            is_aggressor: meta.street_aggressor == Some(j),
            is_live: h.stack[j] > 0,
            barrels: meta.post_barrels[j],
        });
    }

    V2Context {
        hole: v.hole,
        board: v.board,
        bb: v.bb,
        pot: v.pot,
        bet: v.bet,
        stack: v.stack,
        effective: v.effective,
        legal: v.legal,
        pos,
        in_position,
        actors_behind,
        preflop_raises: meta.pf_raises,
        limpers: meta.limpers,
        hero_was_pfa: meta.pfa_seat == Some(seat),
        street_raises: meta.street_raises,
        facing_check_raise: meta.street_raises >= 2
            && meta
                .street_aggressor
                .is_some_and(|a| a != seat && meta.checked_street[a]),
        opponents,
    }
}

fn archetype_action(
    h: &Hand,
    seat: usize,
    arch: Archetype,
    meta: &HandMeta,
    defense: &Defense,
    spent: &mut bool,
    names: &[&str; 6],
    rng: &mut ChaCha8Rng,
) -> Action {
    let v = h.view(seat);
    let l = v.legal;
    let call = l.call.unwrap_or(0).min(v.stack);
    let cost_bb = call as f64 / BB as f64;
    let pot = v.pot.max(1) as f64;
    let score = playability_score(v.hole[0], v.hole[1]);

    match arch {
        Archetype::GtoTag => {
            let ctx = build_v2_context(h, seat, meta, None, names);
            decide_v2_with_params(&ctx, rng.r#gen(), 160, &tag_params()).action
        }
        Archetype::LagShark => {
            let ctx = build_v2_context(h, seat, meta, None, names);
            decide_v2_with_params(&ctx, rng.r#gen(), 160, &lag_params()).action
        }
        Archetype::NitReg => {
            let ctx = build_v2_context(h, seat, meta, None, names);
            decide_v2_with_params(&ctx, rng.r#gen(), 128, &nit_params()).action
        }
        Archetype::PotOddsBot => crate::policy::with_profile(&v, rng.r#gen(), 160, 16).action,
        Archetype::Profile28 => {
            let pressure = if h.street == 0 {
                defense.estimate(
                    h.active()
                        .filter(|&j| j != seat && h.stack[j] > 0)
                        .map(|j| (names[j], h.tight[j] > 0.)),
                )
            } else {
                defense.postflop_pressure(h.active().filter(|&j| j != seat).map(|j| names[j]))
            };
            let d = crate::policy::contextual(&v, rng.r#gen(), 192, 28, *spent, pressure);
            *spent |= d.spends_response();
            d.action
        }
        Archetype::CallingStation => {
            if v.board.is_empty() {
                if score >= 0.92 && let Some((lo, hi)) = l.raise {
                    return Action::Raise(((v.bet + call).max(BB) * 3).clamp(lo, hi));
                }
                if l.check {
                    return Action::Check;
                }
                if l.call.is_some() && (score >= 0.36 || cost_bb <= 4.5 && score >= 0.25 || rng.gen_bool(0.35)) {
                    return Action::Call;
                }
                l.passive().unwrap()
            } else {
                let bi = BoardFast::analyze(&v.board);
                let b = crate::range::postflop_bucket(v.hole, &v.board, mask(&v.board), &bi);
                if b >= 7 && let Some((lo, hi)) = l.raise {
                    let sz = (v.bet + call + (pot * 0.75) as Chips).clamp(lo, hi);
                    return Action::Raise(sz);
                }
                if l.check {
                    if b >= 5 && rng.gen_bool(0.55) && let Some((lo, hi)) = l.raise {
                        return Action::Raise((v.bet + (pot * 0.55) as Chips).clamp(lo, hi));
                    }
                    return Action::Check;
                }
                if l.call.is_some() && (b >= 2 || (b == 1 && call as f64 <= pot * 1.1) || rng.gen_bool(0.22)) {
                    return Action::Call;
                }
                l.passive().unwrap()
            }
        }
        Archetype::LoosePassiveFish => {
            if v.board.is_empty() {
                if score >= 0.88 && let Some((lo, hi)) = l.raise {
                    return Action::Raise(((v.bet + call).max(BB) * 3).clamp(lo, hi));
                }
                if l.check {
                    return Action::Check;
                }
                if l.call.is_some() && (cost_bb <= 1.0 && score >= 0.35 || cost_bb <= 4.0 && score >= 0.50) {
                    return Action::Call;
                }
                l.passive().unwrap()
            } else {
                let bi = BoardFast::analyze(&v.board);
                let b = crate::range::postflop_bucket(v.hole, &v.board, mask(&v.board), &bi);
                if b >= 7 && let Some((lo, hi)) = l.raise {
                    return Action::Raise((v.bet + call + (pot * 0.7) as Chips).clamp(lo, hi));
                }
                if l.check {
                    if b >= 5 && rng.gen_bool(0.65) && let Some((lo, hi)) = l.raise {
                        return Action::Raise((v.bet + (pot * 0.55) as Chips).clamp(lo, hi));
                    }
                    return Action::Check;
                }
                let call_ok = if h.street == 1 {
                    b >= 2 || (b == 1 && call as f64 <= pot * 0.6)
                } else {
                    b >= 4 || (b >= 3 && call as f64 <= pot * 0.6)
                };
                if l.call.is_some() && call_ok {
                    return Action::Call;
                }
                l.passive().unwrap()
            }
        }
        Archetype::Maniac => {
            if v.board.is_empty() {
                if (score >= 0.45 || rng.gen_bool(0.32))
                    && meta.pf_raises < 3
                    && let Some((lo, hi)) = l.raise
                {
                    let to = if meta.pf_raises == 0 {
                        (3.5 * BB as f64) as Chips
                    } else {
                        ((v.bet + call) as f64 * 3.0) as Chips
                    };
                    return Action::Raise(to.clamp(lo, hi));
                }
                if l.check {
                    return Action::Check;
                }
                if l.call.is_some() && (score >= 0.48 || cost_bb <= 8.0 && rng.gen_bool(0.45)) {
                    return Action::Call;
                }
                l.passive().unwrap()
            } else {
                let bi = BoardFast::analyze(&v.board);
                let b = crate::range::postflop_bucket(v.hole, &v.board, mask(&v.board), &bi);
                let fire = (b >= 5 || rng.gen_bool(if call == 0 { 0.52 } else { 0.24 }))
                    && meta.street_raises < 3;
                if fire && let Some((lo, hi)) = l.raise {
                    let frac = if rng.gen_bool(0.35) { 1.15 } else { 0.72 };
                    let to = v.bet + call + ((pot + call as f64) * frac) as Chips;
                    return Action::Raise(to.clamp(lo, hi));
                }
                if l.check {
                    return Action::Check;
                }
                if l.call.is_some() && (b >= 3 || rng.gen_bool(0.28)) {
                    return Action::Call;
                }
                l.passive().unwrap()
            }
        }
        Archetype::FitOrFold => {
            if v.board.is_empty() {
                if meta.pf_raises == 0 && score >= 0.72 && l.raise.is_some() {
                    return Action::Raise((2.5 * BB as f64).round() as Chips).clamp_to(l);
                }
                if l.check {
                    return Action::Check;
                }
                if l.call.is_some() && cost_bb <= 3.5 && score >= 0.62 {
                    return Action::Call;
                }
                l.passive().unwrap()
            } else {
                let bi = BoardFast::analyze(&v.board);
                let b = crate::range::postflop_bucket(v.hole, &v.board, mask(&v.board), &bi);
                if b >= 6 && let Some((lo, hi)) = l.raise {
                    return Action::Raise((v.bet + call + (pot * 0.65) as Chips).clamp(lo, hi));
                }
                if l.check {
                    if b >= 5 && let Some((lo, hi)) = l.raise {
                        return Action::Raise((v.bet + (pot * 0.50) as Chips).clamp(lo, hi));
                    }
                    return Action::Check;
                }
                if l.call.is_some() && (b >= 5 || (b == 4 && call as f64 <= pot * 0.55)) {
                    return Action::Call;
                }
                l.passive().unwrap()
            }
        }
        Archetype::PassiveTrappy => {
            // Matches real OpenPoker bots like Kairo / OMGtheNutsss / minhcasa / Kaiel / vivilma:
            // ~27% VPIP, ~15% PFR, ~1% 3B, ~18% cbet, ~24% fold-to-bet, ~24% small-raise (0.4x) when strong/TP!
            if v.board.is_empty() {
                if meta.pf_raises == 0 && score >= 0.72 && l.raise.is_some() {
                    return Action::Raise((2.5 * BB as f64).round() as Chips).clamp_to(l);
                }
                if meta.pf_raises == 1 && score >= 0.94 && l.raise.is_some() {
                    return Action::Raise(((v.bet + call) as f64 * 2.8) as Chips).clamp_to(l);
                }
                if l.check {
                    return Action::Check;
                }
                if l.call.is_some() && ((cost_bb <= 1.2 && score >= 0.48) || (cost_bb <= 4.0 && score >= 0.54)) {
                    return Action::Call;
                }
                l.passive().unwrap()
            } else {
                let bi = BoardFast::analyze(&v.board);
                let b = crate::range::postflop_bucket(v.hole, &v.board, mask(&v.board), &bi);
                if call == 0 {
                    // Low bet-when-checked-to (~20%): mostly checks unless strong pair+ or value!
                    if (b >= 6 || (b == 5 && rng.gen_bool(0.30))) && l.raise.is_some() {
                        let frac = if b >= 7 { 0.72 } else { 0.58 };
                        return Action::Raise(v.bet + (pot * frac) as Chips).clamp_to(l);
                    }
                    return Action::Check;
                }
                // Facing a bet: rarely folds pairs (f2b ~24%), small-raises (0.4x pot) with TP+/draws (~24% r2b)!
                if (b >= 6 || (b == 5 && rng.gen_bool(0.35)) || (b == 4 && h.street < 3 && rng.gen_bool(0.30)))
                    && meta.street_raises < 2
                    && l.raise.is_some()
                {
                    let to = v.bet + call + ((pot + call as f64) * 0.42) as Chips;
                    return Action::Raise(to).clamp_to(l);
                }
                if l.call.is_some() && (b >= 2 || (b == 1 && call as f64 <= pot * 0.55)) {
                    return Action::Call;
                }
                l.passive().unwrap()
            }
        }
        Archetype::TightFoldy => {
            // Matches Winters / ramses_888: ~21% VPIP, ~13% PFR, ~3.5% 3B, ~35% cbet, ~65% fold-to-bet!
            if v.board.is_empty() {
                if meta.pf_raises == 0 && score >= 0.74 && l.raise.is_some() {
                    return Action::Raise((2.5 * BB as f64).round() as Chips).clamp_to(l);
                }
                if meta.pf_raises == 1 && score >= 0.90 && l.raise.is_some() {
                    return Action::Raise(((v.bet + call) as f64 * 3.0) as Chips).clamp_to(l);
                }
                if l.check {
                    return Action::Check;
                }
                if l.call.is_some() && cost_bb <= 3.5 && score >= 0.66 {
                    return Action::Call;
                }
                l.passive().unwrap()
            } else {
                let bi = BoardFast::analyze(&v.board);
                let b = crate::range::postflop_bucket(v.hole, &v.board, mask(&v.board), &bi);
                if call == 0 {
                    if (b >= 5 || (meta.pfa_seat == Some(seat) && h.street == 1 && rng.gen_bool(0.25)))
                        && l.raise.is_some()
                    {
                        let frac = if b >= 7 { 0.85 } else { 0.60 };
                        return Action::Raise(v.bet + (pot * frac) as Chips).clamp_to(l);
                    }
                    return Action::Check;
                }
                if b >= 7 && meta.street_raises < 2 && l.raise.is_some() {
                    return Action::Raise(v.bet + call + ((pot + call as f64) * 0.85) as Chips).clamp_to(l);
                }
                if l.call.is_some()
                    && (b >= 6 || (b == 5 && call as f64 <= pot * 0.85) || (b == 4 && h.street < 3 && call as f64 <= pot * 0.60))
                {
                    return Action::Call;
                }
                l.passive().unwrap()
            }
        }
        Archetype::HyperLag => {
            // Matches RoRo / Unkn0wn: ~52% VPIP, ~40% PFR, ~10% 3B, ~65% cbet, ~50% f2b, big polar bets!
            if v.board.is_empty() {
                if meta.pf_raises == 0 && (score >= 0.44 || rng.gen_bool(0.12)) && l.raise.is_some() {
                    return Action::Raise((3.0 * BB as f64).round() as Chips).clamp_to(l);
                }
                if meta.pf_raises == 1 && (score >= 0.80 || rng.gen_bool(0.08)) && l.raise.is_some() {
                    return Action::Raise(((v.bet + call) as f64 * 3.2) as Chips).clamp_to(l);
                }
                if l.check {
                    return Action::Check;
                }
                if l.call.is_some() && cost_bb <= 5.0 && score >= 0.44 {
                    return Action::Call;
                }
                l.passive().unwrap()
            } else {
                let bi = BoardFast::analyze(&v.board);
                let b = crate::range::postflop_bucket(v.hole, &v.board, mask(&v.board), &bi);
                if call == 0 {
                    if (b >= 5 || b == 4 || rng.gen_bool(if h.street == 1 { 0.48 } else { 0.24 }))
                        && l.raise.is_some()
                    {
                        let frac = if b >= 7 || rng.gen_bool(0.30) { 1.25 } else { 0.65 };
                        return Action::Raise(v.bet + (pot * frac) as Chips).clamp_to(l);
                    }
                    return Action::Check;
                }
                if (b >= 7 || (b == 4 && h.street < 3 && rng.gen_bool(0.25)))
                    && meta.street_raises < 2
                    && l.raise.is_some()
                {
                    return Action::Raise(v.bet + call + ((pot + call as f64) * 1.10) as Chips).clamp_to(l);
                }
                if l.call.is_some()
                    && (b >= 5 || (b >= 3 && call as f64 <= pot * 0.75) || (b == 4 && h.street < 3))
                {
                    return Action::Call;
                }
                l.passive().unwrap()
            }
        }
    }
}

trait ActionClamp {
    fn clamp_to(self, l: Legal) -> Action;
}
impl ActionClamp for Action {
    fn clamp_to(self, l: Legal) -> Action {
        match self {
            Action::Raise(x) => l
                .raise
                .map(|(lo, hi)| Action::Raise(x.clamp(lo, hi)))
                .unwrap_or_else(|| l.passive().unwrap()),
            a if l.permits(a) => a,
            _ => l.passive().unwrap(),
        }
    }
}

#[derive(Default, Clone)]
pub struct SessionMetrics {
    pub net: i64,
    pub drawdown: i64,
    pub busts: usize,
    pub wins: usize,
    pub vpip: usize,
    pub pfr: usize,
    pub three_bets: usize,
    pub latencies_us: Vec<u64>,
}

fn run_session(
    seed: u64,
    hero: usize,
    length: usize,
    depth: Chips,
    opps: [Archetype; 5],
    hero_profile: u8,
    params: &V2Params,
    samples: usize,
) -> SessionMetrics {
    let buy = depth * BB;
    let mut stacks = [buy; 6];
    let mut hud = HudTracker::default();
    let mut defense = Defense::default();
    let mut rec = SessionMetrics::default();
    let mut peak = 0i64;
    let names = ["seat0", "seat1", "seat2", "seat3", "seat4", "seat5"];

    for t in 0..length {
        let hand_seed = seed + t as u64;
        let mut rng: [_; 6] = std::array::from_fn(|i| {
            ChaCha8Rng::seed_from_u64(hand_seed ^ (i as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15))
        });
        let mut h = Hand::new(6, (hero + t) % 6, stacks, hand_seed);
        let mut meta = HandMeta::default();
        hud.new_hand();
        defense.next_hand();
        for s in 0..6 {
            hud.register_seat(s, names[s], true);
        }
        let mut spent = [false; 6];
        let mut hero_vpip = false;
        let mut hero_pfr = false;
        let mut hero_3bet = false;
        let mut prev_street = 0usize;

        while let Some(i) = h.actor {
            if h.street != prev_street {
                prev_street = h.street;
                hud.on_street_advance(h.street);
                meta.on_street(h.street);
            }
            let v = h.view(i);
            let a = if i == hero {
                let t0 = Instant::now();
                let d: Decision = if hero_profile == crate::v2::V2_PROFILE {
                    let ctx = build_v2_context(&h, i, &meta, Some(&hud), &names);
                    decide_v2_with_params(&ctx, rng[i].r#gen(), samples, params)
                } else {
                    let pressure = if h.street == 0 {
                        defense.estimate(
                            h.active()
                                .filter(|&j| j != hero && h.stack[j] > 0)
                                .map(|j| (names[j], h.tight[j] > 0.)),
                        )
                    } else {
                        defense.postflop_pressure(
                            h.active().filter(|&j| j != hero).map(|j| names[j]),
                        )
                    };
                    let d = crate::policy::contextual(&v, rng[i].r#gen(), samples, hero_profile, spent[i], pressure);
                    spent[i] |= d.spends_response();
                    d
                };
                rec.latencies_us.push(t0.elapsed().as_micros() as u64);
                if h.street == 0 {
                    let call = v.legal.call.unwrap_or(0);
                    let is_r = matches!(a_of(&d), Action::Raise(_))
                        || (matches!(a_of(&d), Action::AllIn) && v.stack > call);
                    if matches!(d.action, Action::Call | Action::Raise(_) | Action::AllIn) {
                        hero_vpip = true;
                    }
                    if is_r {
                        hero_pfr = true;
                        if meta.pf_raises == 1 {
                            hero_3bet = true;
                        }
                    }
                }
                d.action
            } else {
                let arch = opps[(i + 6 - hero - 1) % 6];
                archetype_action(&h, i, arch, &meta, &defense, &mut spent[i], &names, &mut rng[i])
            };

            let (name_act, invested) = match a {
                Action::Raise(to) => ("raise", to.saturating_sub(v.bet)),
                Action::AllIn => ("all_in", v.stack),
                Action::Call => ("call", v.legal.call.unwrap_or(0)),
                Action::Check => ("check", 0),
                Action::Fold => ("fold", 0),
            };
            let owing = v.legal.call.unwrap_or(0);
            let is_r = matches!(a, Action::Raise(_)) || (matches!(a, Action::AllIn) && invested > owing);
            let pot_now: Chips = h.total.iter().sum();

            hud.on_action(i, names[i], h.street, name_act, owing, pot_now, is_r);
            if h.street > 0 {
                defense.end_street();
            }
            let st_name = ["preflop", "flop", "turn", "river"][h.street];
            defense.action(
                &json!({"street":st_name,"name":names[i],"action":name_act,"to_call_before":owing,"contribution_delta":invested,"pot_before":pot_now}),
                Some(hero),
                i,
            );
            meta.record_action(i, h.street, a, &v, h.active().count());
            assert!(h.apply(a));
        }

        let reached_sd = h.active().count() >= 2 && h.street == 3;
        hud.finish_hand(reached_sd);

        let hero_start = stacks[hero];
        let remaining = h.stack[hero];
        let other_max = (0..6).filter(|&j| j != hero).map(|j| h.total[j]).max().unwrap_or(0);
        let refund = h.total[hero].saturating_sub(other_max);
        let end = h.settle();
        let net = i64::from(end[hero]) - i64::from(hero_start);
        rec.net += net;
        peak = peak.max(rec.net);
        rec.drawdown = rec.drawdown.max(peak - rec.net);
        rec.busts += usize::from(end[hero] == 0);
        rec.wins += usize::from(end[hero] > remaining + refund);
        rec.vpip += usize::from(hero_vpip);
        rec.pfr += usize::from(hero_pfr);
        rec.three_bets += usize::from(hero_3bet);
        stacks = end.map(|n| if n == 0 { buy } else { n });
    }

    rec
}

#[inline]
fn a_of(d: &Decision) -> Action {
    d.action
}

fn environments() -> Vec<(&'static str, Chips, [Archetype; 5])> {
    use Archetype::*;
    vec![
        (
            "openpoker_live_meta_250bb",
            250,
            [PassiveTrappy, PassiveTrappy, TightFoldy, HyperLag, CallingStation],
        ),
        (
            "openpoker_live_meta_100bb",
            100,
            [PassiveTrappy, TightFoldy, HyperLag, GtoTag, LoosePassiveFish],
        ),
        (
            "openpoker_pool_250bb",
            250,
            [LagShark, GtoTag, FitOrFold, LoosePassiveFish, CallingStation],
        ),
        (
            "openpoker_pool_50bb",
            50,
            [LagShark, GtoTag, FitOrFold, LoosePassiveFish, CallingStation],
        ),
        (
            "tough_regs_100bb",
            100,
            [LagShark, GtoTag, LagShark, GtoTag, NitReg],
        ),
        (
            "tough_regs_250bb",
            250,
            [LagShark, GtoTag, LagShark, GtoTag, NitReg],
        ),
        (
            "maniac_station_250bb",
            250,
            [Maniac, CallingStation, LagShark, CallingStation, Maniac],
        ),
        (
            "mixed_bots_250bb",
            250,
            [LagShark, PotOddsBot, FitOrFold, CallingStation, Maniac],
        ),
        (
            "vs_profile28_50bb",
            50,
            [Profile28, Profile28, Profile28, Profile28, Profile28],
        ),
        (
            "vs_profile28_250bb",
            250,
            [Profile28, Profile28, Profile28, Profile28, Profile28],
        ),
    ]
}

pub fn bench_v2(sessions: usize, length: usize, seed: u64, samples: usize) -> Value {
    let params = V2Params::embedded();
    bench_v2_with_params(sessions, length, seed, samples, params, true)
}

pub fn bench_v2_with_params(
    sessions: usize,
    length: usize,
    seed: u64,
    samples: usize,
    params: &V2Params,
    include_p28: bool,
) -> Value {
    let t0 = Instant::now();
    let envs = environments();
    let mut out = Vec::new();
    let n = sessions as f64;
    let bb100 = |chips: i64| 100.0 * chips as f64 / (BB as f64 * length as f64);

    for (ei, (name, depth, opps)) in envs.into_iter().enumerate() {
        let base_seed = seed + ei as u64 * 10_000_000;
        let rows: Vec<(SessionMetrics, Option<SessionMetrics>)> = (0..sessions)
            .into_par_iter()
            .map(|r| {
                let s = base_seed + r as u64 * 1000;
                let hero = r % 6;
                let v2 = run_session(s, hero, length, depth, opps, crate::v2::V2_PROFILE, params, samples);
                let p28 = include_p28
                    .then(|| run_session(s, hero, length, depth, opps, 28, params, samples));
                (v2, p28)
            })
            .collect();

        let summarize = |items: Vec<&SessionMetrics>| {
            let means: Vec<f64> = items.iter().map(|m| bb100(m.net)).collect();
            let mean = means.iter().sum::<f64>() / n;
            let se = (means.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0) / n).sqrt();
            let mut dds: Vec<i64> = items.iter().map(|m| m.drawdown).collect();
            dds.sort_unstable();
            let mut lats: Vec<u64> = items.iter().flat_map(|m| m.latencies_us.iter().copied()).collect();
            lats.sort_unstable();
            let tot_h = (sessions * length) as f64;
            let wins: usize = items.iter().map(|m| m.wins).sum();
            let vpip: usize = items.iter().map(|m| m.vpip).sum();
            let pfr: usize = items.iter().map(|m| m.pfr).sum();
            let tb: usize = items.iter().map(|m| m.three_bets).sum();
            let busts = items.iter().filter(|m| m.busts > 0).count();
            json!({
                "bb100": (mean * 100.0).round() / 100.0,
                "ci95": [((mean - 1.96 * se) * 100.0).round() / 100.0, ((mean + 1.96 * se) * 100.0).round() / 100.0],
                "win_pct": (1000.0 * wins as f64 / tot_h).round() / 10.0,
                "vpip_pct": (1000.0 * vpip as f64 / tot_h).round() / 10.0,
                "pfr_pct": (1000.0 * pfr as f64 / tot_h).round() / 10.0,
                "three_bet_pct": (1000.0 * tb as f64 / tot_h).round() / 10.0,
                "p90_drawdown_bb": (dds[(sessions * 9).div_ceil(10) - 1] as f64 / BB as f64 * 10.0).round() / 10.0,
                "bust_pct": (1000.0 * busts as f64 / n).round() / 10.0,
                "p50_us": lats.get(lats.len() / 2).copied().unwrap_or(0),
                "p99_us": lats.get((lats.len().saturating_sub(1)) * 99 / 100).copied().unwrap_or(0),
            })
        };

        let v2_refs: Vec<&SessionMetrics> = rows.iter().map(|(a, _)| a).collect();
        let mut entry = json!({
            "field": name,
            "depth_bb": depth,
            "hands_per_policy": sessions * length,
            "v2": summarize(v2_refs),
        });

        if include_p28 {
            let p28_refs: Vec<&SessionMetrics> = rows.iter().map(|(_, b)| b.as_ref().unwrap()).collect();
            let deltas: Vec<f64> = rows
                .iter()
                .map(|(a, b)| bb100(a.net - b.as_ref().unwrap().net))
                .collect();
            let dmean = deltas.iter().sum::<f64>() / n;
            let dse = (deltas.iter().map(|x| (x - dmean).powi(2)).sum::<f64>() / (n - 1.0).max(1.0) / n).sqrt();
            entry["profile28"] = summarize(p28_refs);
            entry["v2_minus_p28_bb100"] = json!({
                "mean": (dmean * 100.0).round() / 100.0,
                "ci95": [((dmean - 1.96 * dse) * 100.0).round() / 100.0, ((dmean + 1.96 * dse) * 100.0).round() / 100.0],
            });
        }
        out.push(entry);
    }

    json!({
        "sessions_per_field": sessions,
        "hands_per_session": length,
        "samples": samples,
        "elapsed_s": (t0.elapsed().as_secs_f64() * 100.0).round() / 100.0,
        "fields": out,
    })
}

/// Multi-population evolutionary/coordinate optimizer over training seeds.
/// Maximizes a balanced objective across tough reg pools AND realistic OpenPoker fish/maniac pools!
pub fn tune_v2(iterations: usize, sessions: usize, length: usize, seed: u64) -> Value {
    let mut best = V2Params::embedded().clone();
    let score_params = |p: &V2Params, s_seed: u64| -> (f64, Vec<(String, f64)>) {
        // Focus on the 4 core orthogonal environments for fast high-signal training
        use Archetype::*;
        let train_envs: [(&str, Chips, [Archetype; 5], f64); 6] = [
            ("openpoker_live_meta_250bb", 250, [PassiveTrappy, PassiveTrappy, TightFoldy, HyperLag, CallingStation], 0.25),
            ("openpoker_pool_250bb", 250, [LagShark, GtoTag, FitOrFold, LoosePassiveFish, CallingStation], 0.20),
            ("openpoker_pool_50bb", 50, [LagShark, GtoTag, FitOrFold, LoosePassiveFish, CallingStation], 0.10),
            ("tough_regs_250bb", 250, [LagShark, GtoTag, LagShark, GtoTag, NitReg], 0.20),
            ("mixed_bots_250bb", 250, [LagShark, PotOddsBot, FitOrFold, CallingStation, Maniac], 0.13),
            ("vs_profile28_250bb", 250, [Profile28, Profile28, Profile28, Profile28, Profile28], 0.12),
        ];
        let mut obj = 0.0;
        let mut breakdown = Vec::new();
        for (ei, (name, depth, opps, weight)) in train_envs.into_iter().enumerate() {
            let base = s_seed + ei as u64 * 10_000_000;
            let res: Vec<(i64, usize)> = (0..sessions)
                .into_par_iter()
                .map(|r| {
                    let m = run_session(base + r as u64 * 1000, r % 6, length, depth, opps, crate::v2::V2_PROFILE, p, 160);
                    (m.net, m.busts)
                })
                .collect();
            let sum_net: i64 = res.iter().map(|x| x.0).sum();
            let bust_rate = res.iter().filter(|x| x.1 > 0).count() as f64 / sessions as f64;
            let mean_bb100 = 100.0 * sum_net as f64 / (BB as f64 * (sessions * length) as f64);
            // Penalize negative performance in tough_regs and excessive bust rates!
            let mut contrib = if name.starts_with("tough") && mean_bb100 < 0.0 {
                mean_bb100 * 3.0
            } else {
                mean_bb100
            };
            contrib -= (bust_rate - 0.18).max(0.0) * 120.0;
            obj += weight * contrib;
            breakdown.push((name.to_owned(), (mean_bb100 * 100.0).round() / 100.0));
        }
        (obj, breakdown)
    };

    let (mut best_obj, mut best_bd) = score_params(&best, seed);
    let mut history = vec![json!({"iter": 0, "objective": (best_obj * 100.0).round() / 100.0, "breakdown": best_bd})];
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0x51a7_cafe);

    for it in 1..=iterations {
        let mut cand = best.clone();
        let step = if it <= iterations / 2 { 1.0 } else { 0.5 };
        // Perturb 2-3 parameters per trial
        let n_mut = rng.gen_range(2..=3);
        for _ in 0..n_mut {
            let d = rng.gen_range(-1.0..=1.0) * step;
            match rng.gen_range(0..15) {
                0 => cand.open_width_scale = (cand.open_width_scale + 0.08 * d).clamp(0.75, 1.35),
                1 => cand.steal_aggression = (cand.steal_aggression + 0.05 * d).clamp(0.0, 0.30),
                2 => cand.three_bet_scale = (cand.three_bet_scale + 0.12 * d).clamp(0.60, 1.50),
                3 => cand.bb_defend_scale = (cand.bb_defend_scale + 0.08 * d).clamp(0.75, 1.35),
                4 => cand.fold_bias = (cand.fold_bias + 0.04 * d).clamp(-0.18, 0.12),
                5 => cand.bluff_fold_discount = (cand.bluff_fold_discount + 0.05 * d).clamp(0.65, 1.02),
                6 => cand.value_future_bonus = (cand.value_future_bonus + 0.04 * d).clamp(0.0, 0.28),
                7 => cand.ip_eqr = (cand.ip_eqr + 0.03 * d).clamp(0.95, 1.15),
                8 => cand.oop_eqr = (cand.oop_eqr + 0.03 * d).clamp(0.75, 0.98),
                9 => cand.call_ip_eqr = (cand.call_ip_eqr + 0.03 * d).clamp(0.90, 1.10),
                10 => cand.call_oop_eqr = (cand.call_oop_eqr + 0.03 * d).clamp(0.75, 0.98),
                11 => cand.raise_respect_rio = (cand.raise_respect_rio + 0.05 * d).clamp(0.02, 0.35),
                12 => cand.station_value_boost = (cand.station_value_boost + 0.06 * d).clamp(0.0, 0.35),
                13 => cand.cbet_pfa_bonus = (cand.cbet_pfa_bonus + 0.03 * d).clamp(0.0, 0.16),
                _ => cand.draw_implied_scale = (cand.draw_implied_scale + 0.04 * d).clamp(0.04, 0.32),
            }
        }
        let (obj, bd) = score_params(&cand, seed);
        if obj > best_obj + 0.5 {
            best = cand;
            best_obj = obj;
            best_bd = bd;
            history.push(json!({"iter": it, "accepted": true, "objective": (best_obj * 100.0).round() / 100.0, "breakdown": best_bd}));
        }
    }

    json!({
        "best_objective": (best_obj * 100.0).round() / 100.0,
        "best_breakdown": best_bd,
        "best_params": best,
        "history": history,
    })
}

/// Diagnostic trace of V2's biggest losing & winning hands in realistic pools.
pub fn debug_hands(field_idx: usize, seed: u64, hands: usize) -> Value {
    let envs = environments();
    let (name, depth, opps) = envs[field_idx % envs.len()];
    let params = V2Params::embedded();
    let buy = depth * BB;
    let mut stacks = [buy; 6];
    let mut hud = HudTracker::default();
    let mut defense = Defense::default();
    let names = ["seat0", "seat1", "seat2", "seat3", "seat4", "seat5"];
    let hero = 0usize;
    let mut log_hands: Vec<(i64, Value)> = Vec::new();
    let mut reason_stats: std::collections::BTreeMap<&'static str, (usize, i64)> =
        std::collections::BTreeMap::new();

    for t in 0..hands {
        let hand_seed = seed + t as u64;
        let mut rng: [_; 6] = std::array::from_fn(|i| {
            ChaCha8Rng::seed_from_u64(hand_seed ^ (i as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15))
        });
        let mut h = Hand::new(6, (hero + t) % 6, stacks, hand_seed);
        let mut meta = HandMeta::default();
        hud.new_hand();
        defense.next_hand();
        for s in 0..6 {
            hud.register_seat(s, names[s], true);
        }
        let mut spent = [false; 6];
        let mut prev_street = 0usize;
        let mut trace = Vec::new();
        let mut hero_reasons = Vec::new();

        while let Some(i) = h.actor {
            if h.street != prev_street {
                prev_street = h.street;
                hud.on_street_advance(h.street);
                meta.on_street(h.street);
            }
            let v = h.view(i);
            let a = if i == hero {
                let ctx = build_v2_context(&h, i, &meta, Some(&hud), &names);
                let d = decide_v2_with_params(&ctx, rng[i].r#gen(), 256, params);
                hero_reasons.push(d.reason);
                trace.push(json!({
                    "st": h.street,
                    "actor": "HERO",
                    "pos": format!("{:?}", ctx.pos),
                    "pot": v.pot,
                    "call": v.legal.call,
                    "act": format!("{:?}", d.action),
                    "reason": d.reason,
                    "eq": d.equity.map(|x| (x * 1000.0).round() / 1000.0),
                    "n_opp": ctx.opponents.len(),
                }));
                d.action
            } else {
                let arch = opps[(i + 6 - hero - 1) % 6];
                let a = archetype_action(&h, i, arch, &meta, &defense, &mut spent[i], &names, &mut rng[i]);
                trace.push(json!({
                    "st": h.street,
                    "actor": format!("{:?}(s{})", arch, i),
                    "pot": v.pot,
                    "act": format!("{:?}", a),
                }));
                a
            };
            let (name_act, invested) = match a {
                Action::Raise(to) => ("raise", to.saturating_sub(v.bet)),
                Action::AllIn => ("all_in", v.stack),
                Action::Call => ("call", v.legal.call.unwrap_or(0)),
                Action::Check => ("check", 0),
                Action::Fold => ("fold", 0),
            };
            let owing = v.legal.call.unwrap_or(0);
            let is_r = matches!(a, Action::Raise(_)) || (matches!(a, Action::AllIn) && invested > owing);
            let pot_now: Chips = h.total.iter().sum();
            hud.on_action(i, names[i], h.street, name_act, owing, pot_now, is_r);
            if h.street > 0 {
                defense.end_street();
            }
            let st_name = ["preflop", "flop", "turn", "river"][h.street];
            defense.action(
                &json!({"street":st_name,"name":names[i],"action":name_act,"to_call_before":owing,"contribution_delta":invested,"pot_before":pot_now}),
                Some(hero),
                i,
            );
            meta.record_action(i, h.street, a, &v, h.active().count());
            assert!(h.apply(a));
        }
        let reached_sd = h.active().count() >= 2 && h.street == 3;
        hud.finish_hand(reached_sd);
        let hero_start = stacks[hero];
        let hole_hero = [h.holes[hero][0].text(), h.holes[hero][1].text()];
        let board_txt: Vec<String> = h.runout[..[0, 3, 4, 5][h.street]].iter().map(|c| c.text()).collect();
        let opp_holes: Vec<Value> = (1..6)
            .filter(|&j| !h.folded[j])
            .map(|j| json!({"seat": j, "arch": format!("{:?}", opps[j-1]), "hole": [h.holes[j][0].text(), h.holes[j][1].text()]}))
            .collect();
        let end = h.settle();
        let net = i64::from(end[hero]) - i64::from(hero_start);
        for r in &hero_reasons {
            let e = reason_stats.entry(r).or_default();
            e.0 += 1;
            e.1 += net;
        }
        if net <= -(depth as i64 * BB as i64 / 5) || end[hero] == 0 {
            log_hands.push((
                net,
                json!({
                    "hand": t,
                    "net_bb": net as f64 / BB as f64,
                    "start_bb": hero_start as f64 / BB as f64,
                    "end_bb": end[hero] as f64 / BB as f64,
                    "hero": hole_hero,
                    "board": board_txt,
                    "opp_holes": opp_holes,
                    "trace": trace,
                }),
            ));
        }
        stacks = end.map(|n| if n == 0 { buy } else { n });
    }
    log_hands.sort_by_key(|(net, _)| *net);
    let r_json: Vec<Value> = reason_stats
        .into_iter()
        .map(|(k, (c, net))| json!({"reason": k, "count": c, "net_bb": (net as f64 / BB as f64 * 10.0).round() / 10.0, "avg_bb": (net as f64 / BB as f64 / c as f64 * 100.0).round() / 100.0}))
        .collect();
    json!({
        "field": name,
        "hands": hands,
        "reason_stats": r_json,
        "worst_hands": log_hands.into_iter().take(10).map(|(_, v)| v).collect::<Vec<_>>(),
    })
}
