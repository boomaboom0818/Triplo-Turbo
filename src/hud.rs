//! Per-opponent Bayesian HUD tracker using only public table actions.
//! Tracks persistent player names across hands with smooth Beta-Binomial shrinkage.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct OppHud {
    pub hands: f64,
    pub vpip: f64,
    pub pfr: f64,
    pub limp: f64,
    pub three_bet: f64,
    pub fold_to_open: f64,
    pub fold_to_3bet: f64,
    pub cbet_flop: f64,
    pub fold_to_bet: f64,
    pub call_vs_bet: f64,
    pub raise_vs_bet: f64,
    pub agg_freq: f64,
    pub river_agg: f64,
    pub wtsd: f64,
    /// Confidence in [0, 1] that we have meaningful sample size on this opponent.
    pub confidence: f64,
}

impl OppHud {
    /// Prior calibrated to real OpenPoker public 6-max pool (3,854-hand empirical trace:
    /// ~30% VPIP, ~16% PFR, ~3.8% 3-bet, ~27-30% cbet/bet-when-checked-to, ~46% fold-to-bet).
    pub fn pool_prior() -> Self {
        Self {
            hands: 0.0,
            vpip: 0.30,
            pfr: 0.16,
            limp: 0.05,
            three_bet: 0.038,
            fold_to_open: 0.59,
            fold_to_3bet: 0.53,
            cbet_flop: 0.30,
            fold_to_bet: 0.46,
            call_vs_bet: 0.41,
            raise_vs_bet: 0.13,
            agg_freq: 0.28,
            river_agg: 0.25,
            wtsd: 0.31,
            confidence: 0.0,
        }
    }

    /// Stickiness score in [0, 1]: high for calling stations, low for fit-or-fold nits.
    pub fn stickiness(&self) -> f64 {
        let raw = 0.55 * (1.0 - self.fold_to_bet) + 0.30 * self.call_vs_bet + 0.15 * self.wtsd;
        raw.clamp(0.05, 0.95)
    }

    /// Aggression score in [0, 1]: high for maniacs/LAGs, low for passive stations/nits.
    pub fn aggression(&self) -> f64 {
        let raw = 0.45 * self.agg_freq
            + 0.25 * (self.raise_vs_bet * 3.0).min(1.0)
            + 0.15 * (self.three_bet * 5.0).min(1.0)
            + 0.15 * self.river_agg;
        raw.clamp(0.05, 0.95)
    }

    /// Preflop looseness in [0.08, 0.85].
    pub fn looseness(&self) -> f64 {
        self.vpip.clamp(0.08, 0.85)
    }
}

#[derive(Clone, Debug, Default)]
struct Rate {
    num: f64,
    den: f64,
}

impl Rate {
    fn observe(&mut self, hit: bool, decay: f64) {
        self.num = self.num * decay + if hit { 1.0 } else { 0.0 };
        self.den = self.den * decay + 1.0;
    }

    fn posterior(&self, prior_mean: f64, prior_weight: f64) -> f64 {
        (self.num + prior_mean * prior_weight) / (self.den + prior_weight)
    }
}

#[derive(Clone, Debug, Default)]
struct PlayerRecord {
    hands: f64,
    vpip: Rate,
    pfr: Rate,
    limp: Rate,
    three_bet: Rate,
    fold_to_open: Rate,
    fold_to_3bet: Rate,
    cbet_flop: Rate,
    fold_to_bet: Rate,
    call_vs_bet: Rate,
    raise_vs_bet: Rate,
    agg_freq: Rate,
    river_agg: Rate,
    wtsd: Rate,
}

impl PlayerRecord {
    fn summary(&self) -> OppHud {
        let p = OppHud::pool_prior();
        OppHud {
            hands: self.hands,
            vpip: self.vpip.posterior(p.vpip, 10.0),
            pfr: self.pfr.posterior(p.pfr, 10.0),
            limp: self.limp.posterior(p.limp, 10.0),
            three_bet: self.three_bet.posterior(p.three_bet, 14.0),
            fold_to_open: self.fold_to_open.posterior(p.fold_to_open, 10.0),
            fold_to_3bet: self.fold_to_3bet.posterior(p.fold_to_3bet, 10.0),
            cbet_flop: self.cbet_flop.posterior(p.cbet_flop, 10.0),
            fold_to_bet: self.fold_to_bet.posterior(p.fold_to_bet, 12.0),
            call_vs_bet: self.call_vs_bet.posterior(p.call_vs_bet, 12.0),
            raise_vs_bet: self.raise_vs_bet.posterior(p.raise_vs_bet, 12.0),
            agg_freq: self.agg_freq.posterior(p.agg_freq, 14.0),
            river_agg: self.river_agg.posterior(p.river_agg, 12.0),
            wtsd: self.wtsd.posterior(p.wtsd, 12.0),
            confidence: (self.hands / (self.hands + 25.0)).clamp(0.0, 1.0),
        }
    }
}

#[derive(Clone, Debug, Default)]
struct HandSeatState {
    name: String,
    active: bool,
    vpip: bool,
    pfr: bool,
    limp: bool,
    saw_flop: bool,
    folded: bool,
    faced_open: bool,
    folded_to_open: bool,
    three_bet_opp: bool,
    did_three_bet: bool,
    faced_3bet: bool,
    folded_to_3bet: bool,
    cbet_opp: bool,
    did_cbet: bool,
}

#[derive(Clone, Debug, Default)]
pub struct HudTracker {
    players: HashMap<String, PlayerRecord>,
    seats: [HandSeatState; 6],
    pf_raises: u8,
    pfa_seat: Option<usize>,
    flop_bet_seen: bool,
    hand_open: bool,
}

const DECAY: f64 = 0.992; // ~125-hand half-life so we adapt to changing strategies

impl HudTracker {
    pub fn new_hand(&mut self) {
        if self.hand_open {
            self.commit_hand(false);
        }
        self.seats = Default::default();
        self.pf_raises = 0;
        self.pfa_seat = None;
        self.flop_bet_seen = false;
        self.hand_open = true;
    }

    pub fn register_seat(&mut self, seat: usize, name: &str, active: bool) {
        if seat >= 6 || name.is_empty() {
            return;
        }
        let s = &mut self.seats[seat];
        if s.name.is_empty() {
            s.name = name.to_owned();
        }
        s.active |= active;
    }

    pub fn on_street_advance(&mut self, street: usize) {
        if street == 1 {
            for s in &mut self.seats {
                if s.active && !s.folded {
                    s.saw_flop = true;
                }
            }
        }
    }

    /// Observe a public player action. `street`: 0=preflop, 1=flop, 2=turn, 3=river.
    /// `kind`: "fold" | "check" | "call" | "raise" | "all_in"
    pub fn on_action(
        &mut self,
        seat: usize,
        name: &str,
        street: usize,
        kind: &str,
        to_call: u32,
        pot: u32,
        is_raise: bool,
    ) {
        if seat >= 6 || name.is_empty() {
            return;
        }
        self.register_seat(seat, name, true);
        let is_fold = kind == "fold";
        let is_call = kind == "call" || (kind == "all_in" && !is_raise);
        let is_check = kind == "check";
        let is_aggr = is_raise || (kind == "raise") || (kind == "all_in" && is_raise);

        if street == 0 {
            let pf_raises = self.pf_raises;
            let s = &mut self.seats[seat];
            if is_call || is_aggr {
                s.vpip = true;
            }
            if is_aggr {
                s.pfr = true;
            }
            if pf_raises == 0 && is_call && to_call > 0 {
                s.limp = true;
            }
            if pf_raises == 1 && to_call > 0 {
                if !s.faced_open {
                    s.faced_open = true;
                    s.folded_to_open = is_fold;
                }
                if !s.three_bet_opp {
                    s.three_bet_opp = true;
                    s.did_three_bet = is_aggr;
                }
            } else if pf_raises == 2 && to_call > 0 && s.pfr && !s.faced_3bet {
                s.faced_3bet = true;
                s.folded_to_3bet = is_fold;
            }
            if is_fold {
                s.folded = true;
            }
            if is_aggr {
                self.pf_raises = self.pf_raises.saturating_add(1);
                self.pfa_seat = Some(seat);
            }
        } else {
            // Postflop stats: update incrementally for non-trivial pots
            let rec = self.players.entry(name.to_owned()).or_default();
            if street == 1 && !self.flop_bet_seen && self.pfa_seat == Some(seat) && to_call == 0 {
                let s = &mut self.seats[seat];
                if !s.cbet_opp {
                    s.cbet_opp = true;
                    s.did_cbet = is_aggr;
                }
            }
            if is_aggr {
                if street == 1 {
                    self.flop_bet_seen = true;
                }
            }
            if is_fold {
                self.seats[seat].folded = true;
            }
            // Facing a meaningful bet (>= 15% pot)
            if to_call > 0 && pot > 0 && to_call * 7 >= pot {
                rec.fold_to_bet.observe(is_fold, DECAY);
                rec.call_vs_bet.observe(is_call, DECAY);
                rec.raise_vs_bet.observe(is_aggr, DECAY);
            }
            if is_aggr || is_call || is_check {
                rec.agg_freq.observe(is_aggr, DECAY);
                if street == 3 {
                    rec.river_agg.observe(is_aggr, DECAY);
                }
            }
        }
    }

    pub fn finish_hand(&mut self, reached_showdown: bool) {
        if self.hand_open {
            self.commit_hand(reached_showdown);
        }
    }

    fn commit_hand(&mut self, reached_showdown: bool) {
        self.hand_open = false;
        let seats = std::mem::take(&mut self.seats);
        for s in seats {
            if !s.active || s.name.is_empty() {
                continue;
            }
            let rec = self.players.entry(s.name).or_default();
            rec.hands += 1.0;
            rec.vpip.observe(s.vpip, DECAY);
            rec.pfr.observe(s.pfr, DECAY);
            rec.limp.observe(s.limp, DECAY);
            if s.faced_open {
                rec.fold_to_open.observe(s.folded_to_open, DECAY);
            }
            if s.three_bet_opp {
                rec.three_bet.observe(s.did_three_bet, DECAY);
            }
            if s.faced_3bet {
                rec.fold_to_3bet.observe(s.folded_to_3bet, DECAY);
            }
            if s.cbet_opp {
                rec.cbet_flop.observe(s.did_cbet, DECAY);
            }
            if s.saw_flop {
                rec.wtsd.observe(reached_showdown && !s.folded, DECAY);
            }
        }
    }

    pub fn get(&self, name: &str) -> OppHud {
        if name.is_empty() {
            return OppHud::pool_prior();
        }
        self.players
            .get(name)
            .map(PlayerRecord::summary)
            .unwrap_or_else(OppHud::pool_prior)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hud_distinguishes_station_from_nit() {
        let mut hud = HudTracker::default();
        for _ in 0..40 {
            hud.new_hand();
            hud.register_seat(0, "station", true);
            hud.register_seat(1, "nit", true);
            hud.on_action(0, "station", 0, "call", 20, 30, false);
            hud.on_action(1, "nit", 0, "fold", 20, 50, false);
            hud.on_street_advance(1);
            hud.on_action(0, "station", 1, "call", 40, 60, false);
            hud.finish_hand(true);
        }
        let st = hud.get("station");
        let nt = hud.get("nit");
        assert!(st.vpip > 0.75);
        assert!(nt.vpip < 0.12);
        assert!(st.stickiness() > 0.70);
        assert!(st.fold_to_bet < 0.20);
        assert!(st.confidence > 0.55);
    }
}
