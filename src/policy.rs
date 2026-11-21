use crate::cards::{Card, deck, draw, evaluate, mask};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

pub type Chips = u32;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Fold,
    Check,
    Call,
    Raise(Chips),
    AllIn,
}

#[derive(Default, Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Legal {
    pub fold: bool,
    pub check: bool,
    pub call: Option<Chips>,
    pub raise: Option<(Chips, Chips)>,
    pub jam: Option<Chips>,
}
impl Legal {
    pub fn permits(self, a: Action) -> bool {
        match a {
            Action::Fold => self.fold,
            Action::Check => self.check,
            Action::Call => self.call.is_some(),
            Action::AllIn => self.jam.is_some(),
            Action::Raise(n) => self.raise.is_some_and(|(lo, hi)| (lo..=hi).contains(&n)),
        }
    }
    pub fn passive(self) -> Option<Action> {
        [Action::Check, Action::Fold, Action::Call, Action::AllIn]
            .into_iter()
            .find(|&a| self.permits(a))
            .or_else(|| self.raise.map(|(l, _)| Action::Raise(l)))
    }
    fn size(self, target: Chips) -> Option<Action> {
        self.raise
            .map(|(l, h)| Action::Raise(target.clamp(l, h)))
            .or_else(|| self.jam.filter(|&n| n <= target).map(|_| Action::AllIn))
    }
}
#[derive(Clone, Debug)]
pub struct View {
    pub hole: [Card; 2],
    pub board: Vec<Card>,
    pub bb: Chips,
    pub pot: Chips,
    pub bet: Chips,
    pub stack: Chips,
    pub effective: Chips,
    pub late: bool,
    /// Opponent range tightness: zero for unobserved, positive for aggressive lines.
    pub ranges: Vec<f64>,
    pub legal: Legal,
}
#[derive(Debug, Serialize)]
pub struct Decision {
    pub action: Action,
    pub reason: &'static str,
    pub equity: Option<f64>,
    pub pot_odds: f64,
    pub samples: usize,
}
impl Decision {
    pub fn spends_response(&self) -> bool {
        matches!(self.reason, "trained_response" | "sticky_guard_release")
            || self.reason.starts_with("defended_")
    }
}

// Simple continuous ranking for six-max opening/defending ranges.
pub(crate) fn quality(h: [Card; 2]) -> f64 {
    let hi = h[0].rank().max(h[1].rank()) as f64;
    let lo = h[0].rank().min(h[1].rank()) as f64;
    if hi == lo {
        return (0.50 + 0.037 * hi).min(1.0);
    }
    0.041 * hi
        + 0.026 * lo
        + if h[0].suit() == h[1].suit() {
            0.08
        } else {
            0.0
        }
        + if hi - lo <= 2.0 { 0.035 } else { 0.0 }
        - if hi < 12.0 && hi - lo > 3.0 {
            0.045
        } else {
            0.0
        }
}

fn equity(v: &View, samples: usize, rng: &mut ChaCha8Rng, price_ranges: bool) -> f64 {
    let known = mask(&v.hole) | mask(&v.board);
    let mut pool = deck();
    let mut available = 0;
    for c in deck() {
        if c.bit() & known == 0 {
            pool[available] = c;
            available += 1;
        }
    }
    let mut share = 0.0;
    for _ in 0..samples {
        let mut d = pool;
        let mut n = available;
        let mut holdings = [0u64; 5];
        for (i, &t) in v.ranges.iter().enumerate() {
            // Rejected cards are returned to the remaining pool. Strong preflop lines
            // prefer high pairs/broadways; a postflop aggressor also favors made hands.
            let h = loop {
                let before = n;
                let h = [draw(&mut d, &mut n, rng), draw(&mut d, &mut n, rng)];
                let made = if v.board.len() >= 3 {
                    (evaluate(mask(&v.board) | mask(&h)) >> 24).min(3) as f64 / 3.0
                } else {
                    0.0
                };
                let mut weight = (1.0 - t) + t * (0.10 + 0.65 * quality(h).powi(3) + 0.25 * made);
                if price_ranges && v.legal.call.is_some() && (t >= 0.8 || v.ranges.len() == 1) {
                    let (tier, _, draw) = strength(h, &v.board);
                    let large = v.legal.call.unwrap_or(0) as f64 > v.pot as f64 * 0.36;
                    weight *= match tier {
                        3 => 1.0,
                        2 => {
                            if large {
                                0.58
                            } else {
                                0.84
                            }
                        }
                        1 => {
                            if large {
                                0.16
                            } else {
                                0.38
                            }
                        }
                        _ if draw => 0.25,
                        _ => {
                            if large {
                                0.045
                            } else {
                                0.11
                            }
                        }
                    };
                }
                if rng.r#gen::<f64>() < weight {
                    break h;
                }
                n = before;
            };
            holdings[i] = mask(&h);
        }
        let mut board = mask(&v.board);
        for _ in v.board.len()..5 {
            board |= draw(&mut d, &mut n, rng).bit();
        }
        let hero = evaluate(board | mask(&v.hole));
        let mut ties = 1;
        for &h in &holdings[..v.ranges.len()] {
            let other = evaluate(board | h);
            if other > hero {
                ties = 0;
                break;
            }
            if other == hero {
                ties += 1;
            }
        }
        if ties > 0 {
            share += 1.0 / ties as f64;
        }
    }
    share / samples as f64
}

/// Private made strength: board-only, weak pair, top pair/overpair, two pair+.
fn strength(h: [Card; 2], b: &[Card]) -> (u8, bool, bool) {
    let board = mask(b);
    let bits = board | mask(&h);
    let score = evaluate(bits);
    let cat = score >> 24;
    let board_rank = if b.len() == 5 { evaluate(board) } else { 0 };
    let mut rank = [0u8; 15];
    for c in b {
        rank[c.rank() as usize] += 1;
    }
    let board_pairs = rank.iter().filter(|&&k| k == 2).count();
    let board_trips = rank.iter().any(|&k| k >= 3);
    let top = b.iter().map(|x| x.rank()).max().unwrap();
    let own = h.map(Card::rank);
    let p = own[0] == own[1];
    let tier = if score == board_rank {
        0
    } else if cat >= 4 || cat == 3 && !board_trips || cat == 2 && board_pairs == 0 {
        3
    } else if (cat == 1 || cat == 2) && (own.contains(&top) || p && own[0] > top) {
        2
    } else if p || own.iter().any(|&x| rank[x as usize] > 0) {
        1
    } else {
        0
    };
    let draw = b.len() < 5
        && (0..4).any(|s| {
            ((bits >> (13 * s)) & 8191).count_ones() == 4
                && h.iter().any(|c| c.suit() as usize == s)
        });
    (tier, score == board_rank, draw)
}

pub const DEFAULT_PROFILE: u8 = 20;
pub const ADAPTIVE: u8 = 21;
pub const GUARDED_ADAPTIVE: u8 = 22;
pub const STICKY_LAB_PROFILE: u8 = 28;
pub const CONTEXT_PRICE_PROFILE: u8 = 34;
pub const MAX_LAB_PROFILE: u8 = CONTEXT_PRICE_PROFILE;

pub fn uses_postflop_history(profile: u8) -> bool {
    matches!(profile, STICKY_LAB_PROFILE..=MAX_LAB_PROFILE)
}

pub fn uses_history(profile: u8) -> bool {
    matches!(profile, ADAPTIVE..=MAX_LAB_PROFILE)
}

/// Counterfactual options for first-in/limped pots only. Value opens and
/// reraises are outside the fitted table. All sizes obey the existing risk cap.
pub fn steal_options(
    v: &View,
    base: Action,
    h: crate::defense::Pressure,
) -> Option<(usize, [Option<Action>; 4])> {
    let l = v.legal;
    if !h.supported
        || !v.board.is_empty()
        || v.bet > v.bb
        || v.bet.saturating_add(l.call.unwrap_or(0)) > v.bb
        || !matches!(base, Action::Raise(_))
    {
        return None;
    }
    let q = quality(v.hole);
    let pair = v.hole[0].rank() == v.hole[1].rank();
    if q >= 0.87 || pair && v.hole[0].rank() >= 9 {
        return None;
    }
    let risk = if h.fold_through < 0.38 || h.reraise > 0.24 {
        0
    } else if h.fold_through < 0.60 || h.reraise > 0.13 {
        1
    } else {
        return None;
    };
    let strength = if q < 0.55 {
        0
    } else if q < 0.72 && !pair {
        1
    } else {
        2
    };
    let cap = v.stack.min(v.effective) / 3;
    let size = |bb: u32| {
        l.size(v.bb.saturating_mul(bb)).filter(|a|matches!(a,Action::Raise(n) if n.saturating_sub(v.bet)<=cap && *n<=v.bb.saturating_mul(12)))
    };
    let passive = if l.check {
        Some(Action::Check)
    } else {
        l.fold.then_some(Action::Fold)
    };
    let limp = if l.check {
        Some(Action::Check)
    } else {
        l.call.map(|_| Action::Call)
    };
    Some((
        risk * 6 + strength * 2 + usize::from(v.pot > v.bb.saturating_mul(2)),
        [passive, limp, size(5), size(9)],
    ))
}

pub fn contextual(
    v: &View,
    seed: u64,
    samples: usize,
    profile: u8,
    used: bool,
    history: crate::defense::Pressure,
) -> Decision {
    if matches!(profile, 29..=31) && v.board.is_empty() {
        let soft = profile == 31
            && history.supported
            && history.fold_through >= 0.50
            && history.reraise <= 0.16
            && v.bet.saturating_add(v.legal.call.unwrap_or(0)) <= v.bb
            && (v.late || v.ranges.len() <= 2);
        if !soft {
            return crate::preflop::priced(v, profile >= 30);
        }
    }
    let mut d = with_response_guard(
        v,
        seed,
        samples,
        budgeted_profile(if uses_history(profile) { 20 } else { profile }, used),
        profile,
        history.sticky_postflop,
    );
    if uses_history(profile)
        && !used
        && let Some((key, opts)) = steal_options(v, d.action, history)
    {
        static CHOICES: std::sync::OnceLock<[Option<usize>; 12]> = std::sync::OnceLock::new();
        let choices = CHOICES.get_or_init(|| {
            #[derive(Deserialize)]
            struct Model {
                base: u8,
                choices: std::collections::BTreeMap<usize, usize>,
            }
            let m: Model = serde_json::from_str(include_str!("../models/adaptive.json")).unwrap();
            assert_eq!(m.base, 20);
            let mut choices = [None; 12];
            for (k, i) in m.choices {
                assert!(i < 4);
                choices[k] = Some(i);
            }
            choices
        });
        if let Some(a) = choices[key].and_then(|i| opts[i])
            && a != d.action
            && v.legal.permits(a)
        {
            d.action = a;
            d.reason = match a {
                Action::Raise(_) => "defended_resize",
                Action::Call => "defended_limp",
                _ => "defended_pass",
            };
        }
    }
    if (matches!(profile, 32 | 33) || profile == CONTEXT_PRICE_PROFILE && history.disciplined_field)
        && let Some(action) = crate::preflop::speculative_call_guard(v, &d, profile == 33)
    {
        d.action = action;
        d.reason = "guarded_preflop_price";
    }
    d
}
pub fn budgeted_profile(p: u8, used: bool) -> u8 {
    if p >= 19 && used { 17 } else { p }
}
#[cfg(test)]
fn decide(v: &View, seed: u64, samples: usize) -> Decision {
    with_profile(v, seed, samples, DEFAULT_PROFILE)
}
pub fn with_profile(v: &View, seed: u64, samples: usize, profile: u8) -> Decision {
    with_response_guard(v, seed, samples, profile, profile, false)
}

fn with_response_guard(
    v: &View,
    seed: u64,
    samples: usize,
    profile: u8,
    variant: u8,
    sticky: bool,
) -> Decision {
    if (18..=20).contains(&profile) {
        static MODEL: std::sync::OnceLock<std::collections::BTreeMap<u32, usize>> =
            std::sync::OnceLock::new();
        let mut d = with_profile(v, seed, samples, 17);
        if let Some((k, options)) = response_probe(v, &d) {
            let actions = MODEL.get_or_init(|| {
                #[derive(Deserialize)]
                struct M {
                    base_profile: u8,
                    actions: std::collections::BTreeMap<u32, usize>,
                }
                let m: M = serde_json::from_str(include_str!("../models/response.json")).unwrap();
                assert_eq!(m.base_profile, 17);
                m.actions
            });
            if let Some(a) = actions
                .get(&k)
                .and_then(|&i| options.get(i))
                .copied()
                .flatten()
                && (profile == 18
                    || robust_allowed(v, &d, a)
                        && (profile == 20 || robust_keys().binary_search(&k).is_ok()))
            {
                let sticky_exception = uses_postflop_history(variant)
                    && sticky
                    && guard_release_bucket(v, &d).is_some()
                    && matches!(a, Action::Raise(_) | Action::AllIn);
                if (GUARDED_ADAPTIVE..=MAX_LAB_PROFILE).contains(&variant)
                    && veto_multiway_bluff(
                        v,
                        &d,
                        a,
                        matches!(variant, 24 | 25) || uses_postflop_history(variant) && sticky,
                    )
                    && !(variant == 27
                        && guard_release_bucket(v, &d).is_some_and(guard_release_allowed))
                {
                    // Keep the base action and leave the one-per-hand override available.
                    d.reason = "guarded_multiway_check";
                } else if matches!(variant, 23 | 25) && veto_early_steal(v, &d, a) {
                    d.reason = "guarded_early_steal";
                } else if variant == 26 && veto_premium_limp(v, &d, a) {
                    d.reason = "guarded_premium_raise";
                } else {
                    d.action = a;
                    d.reason = if sticky_exception {
                        "sticky_guard_release"
                    } else {
                        "trained_response"
                    };
                }
            }
        }
        return d;
    }
    if profile == 17 {
        static TABLE: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
        let mut base = profit_policy(v, seed, samples, 14);
        if let Some((bucket, alternative)) = probe(v, base.action) {
            let enabled = TABLE.get_or_init(|| {
                #[derive(Deserialize)]
                struct Model {
                    base_profile: u8,
                    enabled: Vec<u32>,
                }
                let model: Model =
                    serde_json::from_str(include_str!("../models/preflop.json")).unwrap();
                assert_eq!(model.base_profile, 14);
                model.enabled
            });
            if enabled.binary_search(&bucket).is_ok() {
                base.action = alternative;
                base.reason = "rollout_preflop";
            }
        }
        return base;
    }
    if profile >= 11 {
        return profit_policy(v, seed, samples, profile);
    }
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let l = v.legal;
    let call = l.call.unwrap_or(0).min(v.stack);
    let odds = call as f64 / (v.pot as f64 + call as f64).max(1.0);
    let bb = v.bb.max(1) as f64;
    let n = v.ranges.len();
    let q = quality(v.hole);
    let price = call as f64 / bb;
    let fallback = l.passive().expect("nonempty legal menu");
    let mut eq = None;
    let mut count = 0;
    let (action, reason) = if v.board.is_empty() {
        let widening = [
            0.0, 0.065, 0.13, 0.19, 0.16, 0.20, 0.24, 0.23, 0.27, 0.31, 0.31,
        ][profile as usize];
        let open =
            0.76 - widening - if v.late { 0.09 } else { 0.0 } - if n <= 2 { 0.09 } else { 0.0 };
        let hi = v.hole[0].rank().max(v.hole[1].rank());
        let lo = v.hole[0].rank().min(v.hole[1].rank());
        let premium = (hi == lo && hi >= 12) || (hi == 14 && lo == 13);
        let suited = v.hole[0].suit() == v.hole[1].suit();
        let playable_draw = suited && (hi - lo <= 3 || hi == 14) || hi == lo;
        let iso = profile >= 7
            && v.late
            && v.bet as f64 <= bb
            && (1.0..=3.5).contains(&price)
            && n <= 3
            && (hi == 14 && suited || hi == lo && hi >= 10)
            && rng.gen_bool(0.60);
        let raise = if price <= 1.0 && q >= open {
            let base = if profile >= 4 && !premium { 2.15 } else { 2.5 };
            Some((base * bb + 0.65 * (v.pot as f64 - 1.5 * bb).max(0.0)) as Chips)
        } else if iso || premium && (price <= 12.0 || (hi == lo && hi >= 13)) {
            Some(if price > 12.0 || v.effective as f64 / bb < 20.0 {
                v.bet.saturating_add(v.effective)
            } else {
                ((v.bet as f64 + call as f64) * 3.2) as Chips
            })
        } else {
            None
        };
        if let Some(a) = raise.and_then(|x| l.size(x)) {
            (a, "preflop_raise")
        } else if l.check {
            (Action::Check, "free_option")
        } else if l.call.is_some()
            && (premium && (price < 25.0 || n <= 1 || (hi == lo && hi >= 13))
                || profile == 10
                    && price <= 1.0
                    && odds <= 0.22
                    && v.effective as f64 >= price * bb * 12.0
                || profile >= 7
                    && v.bet as f64 <= bb
                    && price <= 3.0
                    && odds < 0.31
                    && (playable_draw || q > 0.64 && v.late)
                || q >= open + 0.7 * widening + 0.035 * price.log2().max(0.0)
                    && price <= 7.0
                    && odds < 0.45)
        {
            (Action::Call, "preflop_defend")
        } else {
            (fallback, "outside_range")
        }
    } else {
        let e = equity(v, samples, &mut rng, false);
        eq = Some(e);
        count = samples;
        let spr = v.effective as f64 / (v.pot.max(1) as f64);
        let pressure = price > 5.0 && call as f64 > 0.55 * v.pot as f64;
        let guarded = profile >= 4;
        let big_pot = v.pot as f64 > bb * 12.0;
        let value = e > if call > 0 {
            0.78 + 0.04 * pressure as u8 as f64 + if guarded && big_pot { 0.10 } else { 0.0 }
        } else {
            0.61 + 0.025 * n as f64
        };
        let bluff = if guarded {
            l.check
                && n <= 2
                && spr >= 1.8
                && v.board.len() < 5
                && !big_pot
                && e > 0.27
                && !value
                && rng.gen_bool(if n == 1 { 0.60 } else { 0.20 })
        } else {
            l.check
                && v.late
                && n == 1
                && v.board.len() == 3
                && (0.28..0.56).contains(&e)
                && rng.gen_bool(0.17 + 0.12 * profile as f64)
        };
        let fraction = if bluff {
            0.34
        } else if guarded && e > 0.95 && n == 1 {
            1.1
        } else if e > 0.88 {
            0.88
        } else if n >= 3 {
            0.68
        } else {
            0.50
        };
        let add = call as f64 + fraction * (v.pot as f64 + call as f64);
        let size = if value && spr < 1.25 {
            v.effective as f64
        } else {
            add.min(v.effective as f64)
        };
        if let Some(a) = (value || bluff)
            .then(|| l.size(v.bet.saturating_add(size as Chips)))
            .flatten()
        {
            (
                a,
                if bluff {
                    "flop_semibluff"
                } else {
                    "equity_value"
                },
            )
        } else if l.check {
            (Action::Check, "free_option")
        } else if l.call.is_some()
            && e > odds
                + 0.035
                + if pressure { 0.12 } else { 0.0 }
                + if guarded && big_pot {
                    if v.board.len() == 5 { 0.24 } else { 0.15 }
                } else {
                    0.0
                }
                + if guarded && v.bet > 0 && call > v.bet {
                    0.10
                } else {
                    0.0
                }
                + if v.board.len() < 5 && !v.late {
                    0.045
                } else {
                    0.0
                }
        {
            (Action::Call, "priced_call")
        } else {
            (fallback, "price_too_high")
        }
    };
    debug_assert!(l.permits(action));
    Decision {
        action,
        reason,
        equity: eq,
        pot_odds: odds,
        samples: count,
    }
}

fn veto_multiway_bluff(v: &View, base: &Decision, proposed: Action, relative: bool) -> bool {
    let floor = if relative {
        0.75 / (v.ranges.len() + 1) as f64
    } else {
        0.25
    };
    !v.board.is_empty()
        && v.ranges.len() >= 2
        && base.action == Action::Check
        && base.equity.is_some_and(|e| e < floor)
        && matches!(proposed, Action::Raise(_) | Action::AllIn)
}

/// Observable bins for the only disagreement between the fixed and relative guards.
pub fn guard_release_bucket(v: &View, base: &Decision) -> Option<usize> {
    let equity = base.equity?;
    if v.board.is_empty()
        || v.ranges.len() < 3
        || base.action != Action::Check
        || !(0.75 / (v.ranges.len() + 1) as f64..0.25).contains(&equity)
    {
        return None;
    }
    let later = usize::from(v.board.len() > 3);
    let compressed = usize::from(v.effective < v.pot.saturating_mul(2));
    let aggressive = usize::from(v.ranges.iter().any(|&r| r >= 0.8));
    Some(later * 4 + compressed * 2 + aggressive)
}

fn guard_release_allowed(bucket: usize) -> bool {
    static MASK: std::sync::OnceLock<[bool; 8]> = std::sync::OnceLock::new();
    MASK.get_or_init(|| {
        let mut mask = [false; 8];
        for i in serde_json::from_str::<Vec<usize>>(include_str!("../models/guard-release.json"))
            .unwrap()
        {
            mask[i] = true;
        }
        mask
    })[bucket]
}

fn veto_early_steal(v: &View, base: &Decision, proposed: Action) -> bool {
    v.board.is_empty()
        && !v.late
        && v.ranges.len() >= 3
        && v.bet.saturating_add(v.legal.call.unwrap_or(0)) <= v.bb
        && matches!(base.action, Action::Check | Action::Fold)
        && matches!(proposed, Action::Raise(_) | Action::AllIn)
        && quality(v.hole) < 0.62
}

fn veto_premium_limp(v: &View, base: &Decision, proposed: Action) -> bool {
    let [a, b] = v.hole.map(Card::rank);
    let premium = (a == b && a >= 12) || a.min(b) == 13 && a.max(b) == 14;
    v.board.is_empty()
        && premium
        && v.bet.saturating_add(v.legal.call.unwrap_or(0)) <= v.bb
        && matches!(base.action, Action::Raise(_) | Action::AllIn)
        && matches!(proposed, Action::Call | Action::Check | Action::Fold)
}

fn robust_keys() -> &'static [u32] {
    static K: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        #[derive(Deserialize)]
        struct M {
            enabled: Vec<u32>,
        }
        serde_json::from_str::<M>(include_str!("../models/robust.json"))
            .unwrap()
            .enabled
    })
}

pub fn robust_allowed(v: &View, d: &Decision, a: Action) -> bool {
    let risk = match a {
        Action::Raise(x) => x.saturating_sub(v.bet),
        Action::AllIn => v.stack,
        Action::Call => v.legal.call.unwrap_or(0),
        _ => 0,
    };
    if v.board.is_empty() {
        let premium = v.hole[0].rank() == v.hole[1].rank() && v.hole[0].rank() >= 12;
        v.bet <= v.bb && (premium || risk <= v.bb.saturating_mul(12) && risk <= v.stack / 3)
    } else {
        v.bet == 0 && (d.equity.unwrap_or(0.0) > 0.82 || risk <= v.pot && risk <= v.stack / 3)
    }
}

/// Stable action-coordinate alternatives for offline rollouts against trained bots.
/// Index 0 is passive, 1 is a pressure size, 2 is an overbet/value size.
pub fn response_probe(v: &View, d: &Decision) -> Option<(u32, [Option<Action>; 3])> {
    let l = v.legal;
    let bb = v.bb as f64;
    let call = l.call.unwrap_or(0);
    let c = call as f64 / bb;
    let q = quality(v.hole);
    let tag = match d.action {
        Action::Fold => 0,
        Action::Check => 1,
        Action::Call => 2,
        Action::Raise(_) | Action::AllIn => 3,
    };
    let (key, opts) = if v.board.is_empty() {
        let high = v.hole.iter().map(|c| c.rank()).max()?;
        let pair = v.hole[0].rank() == v.hole[1].rank();
        let strength = if pair && high >= 13 {
            5
        } else if pair && high >= 10 || q >= 0.90 {
            4
        } else if q >= 0.79 {
            3
        } else if q >= 0.67 {
            2
        } else if q >= 0.54 {
            1
        } else {
            0
        };
        let cost = if c <= 1. {
            0
        } else if c <= 4. {
            1
        } else if c <= 12. {
            2
        } else {
            3
        };
        let raise = v.bet.saturating_add(call).max(v.bb);
        let pressure = if cost == 0 {
            3 * v.bb + v.pot / 2
        } else {
            3 * raise + v.pot / 3
        };
        let big = if strength >= 4 {
            (6 * raise + v.pot).min(v.effective + v.bet)
        } else {
            4 * raise + v.pot / 2
        };
        (
            1000 + ((tag * 4 + cost) * 6 + strength) * 2 + u32::from(v.late),
            [
                if matches!(d.action, Action::Call) && cost > 0 {
                    l.fold.then_some(Action::Fold)
                } else if l.check {
                    Some(Action::Check)
                } else {
                    l.call.map(|_| Action::Call)
                },
                l.size(pressure),
                l.size(big),
            ],
        )
    } else {
        if d.reason == "locked_split" {
            return None;
        }
        let e = d.equity?;
        let tier = if e < 0.25 {
            0
        } else if e < 0.5 {
            1
        } else if e < 0.75 {
            2
        } else if e < 0.92 {
            3
        } else {
            4
        };
        let cost = if call == 0 {
            0
        } else if c <= 5.0 {
            1
        } else {
            2
        };
        let street = if v.board.len() == 5 { 1 } else { 0 };
        let to = |f: f64| {
            v.bet.saturating_add(
                (call as f64 + f * (v.pot as f64 + call as f64)).min(v.effective as f64) as Chips,
            )
        };
        (
            2000 + (((tag * 3 + cost) * 5 + tier) * 2 + street) * 2
                + u32::from(v.ranges.len() == 1),
            [
                if matches!(d.action, Action::Call) {
                    l.fold.then_some(Action::Fold)
                } else if l.check {
                    Some(Action::Check)
                } else {
                    l.call.map(|_| Action::Call)
                },
                l.size(to(0.6)),
                l.size(to(if e > 0.85 { 2.5 } else { 1.35 })),
            ],
        )
    };
    let clean = opts.map(|o| o.filter(|&a| a != d.action && l.permits(a)));
    clean.iter().any(Option::is_some).then_some((key, clean))
}

/// One alternative per observable preflop bucket. The trainer compares full-hand
/// payouts on identical hidden deals; only supported, positive lower bounds ship.
pub fn probe(v: &View, base: Action) -> Option<(u32, Action)> {
    if !v.board.is_empty() || v.bb == 0 || v.bet > v.bb || v.effective < 16 * v.bb {
        return None;
    }
    let q = quality(v.hole);
    let call = v.legal.call.unwrap_or(0);
    let cost = call as f64 / v.bb as f64;
    if cost > 6.0 {
        return None;
    }
    let family = if cost > 1.0 {
        2
    } else if v.pot as f64 > 1.9 * v.bb as f64 {
        1
    } else {
        0
    };
    let (tag, alt) = match base {
        Action::Raise(_) if q >= 0.87 => {
            (0, v.legal.size((v.bet + call).max(v.bb) * 6 + v.pot / 3)?)
        }
        Action::Raise(_) if family == 2 => (1, Action::Call),
        Action::Raise(_) if family == 1 => (
            2,
            if v.legal.check {
                Action::Check
            } else {
                Action::Call
            },
        ),
        Action::Raise(_) if family == 0 => (3, v.legal.size(4 * v.bb)?),
        Action::Fold if family < 2 && (q >= 0.42 || v.late) => (
            4,
            if family == 0 {
                v.legal.size(3 * v.bb)?
            } else {
                Action::Call
            },
        ),
        Action::Fold if family == 2 && q >= 0.68 => (5, Action::Call),
        Action::Call if family == 2 && q >= 0.8 => {
            (6, v.legal.size((v.bet + call) * 3 + v.pot / 3)?)
        }
        Action::Call if family == 2 => (7, Action::Fold),
        Action::Call if family <= 1 => (8, v.legal.size(3 * v.bb + v.pot / 2)?),
        _ => return None,
    };
    if alt == base || !v.legal.permits(alt) {
        return None;
    }
    let strength = if q >= 0.91 {
        3
    } else if q >= 0.79 {
        2
    } else if q >= 0.65 {
        1
    } else {
        0
    };
    let key = (((tag * 3 + family) * 4 + strength) * 2 + u32::from(v.late)) * 2
        + u32::from(v.ranges.len() <= 2);
    Some((key, alt))
}

fn profit_policy(v: &View, seed: u64, samples: usize, p: u8) -> Decision {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let l = v.legal;
    let call = l.call.unwrap_or(0).min(v.stack);
    let odds = call as f64 / (call as f64 + v.pot as f64).max(1.0);
    let bb = v.bb.max(1) as f64;
    let cost = call as f64 / bb;
    let price = call as f64 / v.pot.max(1) as f64;
    let n = v.ranges.len();
    let depth = v.effective as f64 / bb;
    let passive = l.passive().unwrap();
    let mut d = Decision {
        action: passive,
        reason: "decline_price",
        equity: None,
        pot_odds: odds,
        samples: 0,
    };
    let cont = if l.check {
        Action::Check
    } else if l.call.is_some() {
        Action::Call
    } else {
        passive
    };
    if v.board.is_empty() {
        let hi = v.hole[0].rank().max(v.hole[1].rank());
        let lo = v.hole[0].rank().min(v.hole[1].rank());
        let pair = hi == lo;
        let suited = v.hole[0].suit() == v.hole[1].suit();
        let q = quality(v.hole);
        let premium = pair && hi >= 12 || hi == 14 && lo == 13;
        let medium = pair && hi >= 9 || hi == 14 && lo >= 12 || hi == 13 && lo == 12 && suited;
        let cutoff = match p {
            11 | 14 => 0.71,
            12 => 0.64,
            15 => 0.77,
            16 => 0.68,
            _ => 0.57,
        } - if v.late { 0.10 } else { 0.0 }
            - if n <= 2 { 0.10 } else { 0.0 };
        if cost <= 1.0 && (q >= cutoff || pair || suited && hi == 14) {
            let iso = ((v.pot as f64 - 1.5 * bb) / bb).max(0.0);
            let base = if p >= 14 {
                if premium {
                    5.0
                } else if medium {
                    3.5
                } else {
                    2.25
                }
            } else if premium {
                3.3
            } else {
                2.4
            };
            let to = ((base + 0.85 * iso) * bb) as Chips;
            d.action = l.size(to).unwrap_or(cont);
            d.reason = "open_or_isolate";
        } else if (premium || p >= 14 && pair && hi >= 10 && cost <= 3.5)
            && (cost <= 12.0 || pair && hi == 14 || pair && hi == 13 && (depth < 60.0 || n == 1))
        {
            let to = if (cost > 9.0 || depth < 21.0) && (pair || depth < 30.0) {
                v.effective.saturating_add(v.bet)
            } else {
                ((v.bet + call) as f64
                    * if p >= 14 && hi == lo && hi >= 13 {
                        4.5
                    } else if v.late {
                        3.0
                    } else {
                        3.65
                    }) as Chips
            };
            d.action = l.size(to).unwrap_or(cont);
            d.reason = "premium_value";
        } else if p >= 14
            && cost <= 1.0
            && odds <= 0.30
            && depth > 20.0
            && (suited || hi - lo <= 3 || hi >= 13)
        {
            d.action = cont;
            d.reason = "cheap_flop";
        } else if l.check
            || (premium && cost <= if pair { 22.0 } else { 14.0 })
            || medium && cost <= 5.5 && odds <= 0.40
            || cost <= 3.5
                && odds < 0.33
                && (pair && depth >= 12.0 * cost
                    || suited && hi - lo <= 2 && depth > 20.0 * cost
                    || v.late && q >= 0.77)
            || cost <= 1.0 && odds < 0.21 && (suited || hi - lo <= 2)
        {
            d.action = cont;
            d.reason = "priced_defend";
        }
        return d;
    }
    let (tier, board_plays, draw) = strength(v.hole, &v.board);
    let street = v.board.len();
    let e = equity(v, samples, &mut rng, true);
    d.equity = Some(e);
    d.samples = samples;
    // Enumerate remaining river holdings only when the simulation sees no loss or the board plays.
    let nuts = street == 5 && (e > 0.94 || board_plays) && {
        let b = mask(&v.board);
        let k = b | mask(&v.hole);
        let h = evaluate(k);
        let rest: Vec<_> = deck().into_iter().filter(|c| c.bit() & k == 0).collect();
        !rest.iter().enumerate().any(|(i, c)| {
            rest[..i]
                .iter()
                .any(|o| evaluate(b | c.bit() | o.bit()) > h)
        })
    };
    if nuts && board_plays {
        d.action = cont;
        d.reason = "locked_split";
        return d;
    }
    let spr = v.effective as f64 / v.pot.max(1) as f64;
    let big = v.pot as f64 > 16.0 * bb;
    let raised = v.bet > 0 && call > 0;
    let value = nuts
        || !board_plays
            && if call > 0 {
                tier >= 3 && e > if raised { 0.89 } else { 0.78 }
            } else {
                (tier >= 3 && e > 0.57 || tier >= 2 && e > 0.64 || e > 0.79)
                    && (!big || tier >= 3 || e > 0.86)
            };
    let semi = call == 0
        && !big
        && spr > 2.0
        && n <= 2
        && (draw || n == 1 && v.late && (0.30..0.58).contains(&e))
        && rng.gen_bool(if p == 11 || p >= 14 { 0.22 } else { 0.45 });
    let calling = if street == 5 {
        e
    } else {
        e * if v.late { 0.94 } else { 0.89 }
    };
    let threshold = odds
        + 0.026
        + if raised && tier < 3 { 0.13 } else { 0.0 }
        + if big && tier <= 1 { 0.13 } else { 0.0 }
        + if price > 0.42 && tier < 3 { 0.10 } else { 0.0 };
    if value || semi {
        let f = if semi {
            0.40
        } else if p >= 14 && (nuts || e > 0.91 && tier >= 3) {
            if street == 5 { 2.0 } else { 1.35 }
        } else if nuts || tier >= 3 && e > 0.91 {
            if p == 13 { 1.4 } else { 1.0 }
        } else if tier >= 3 {
            0.78
        } else {
            0.55
        };
        let invest = if value && (nuts || tier >= 3 && e > 0.84) && spr < 1.5 {
            v.effective
        } else {
            (call as f64 + f * (v.pot as f64 + call as f64)).min(v.effective as f64) as Chips
        };
        if let Some(a) = l.size(v.bet.saturating_add(invest)) {
            d.action = a;
            d.reason = if semi && !value {
                "draw_pressure"
            } else {
                "private_value"
            };
            return d;
        }
    }
    if l.check
        || nuts
        || !board_plays && calling >= threshold
        || board_plays && calling >= odds + 0.04
    {
        d.action = cont;
        d.reason = if l.check {
            "check"
        } else {
            "range_priced_call"
        };
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guarded_profile_checks_instead_of_injecting_a_low_equity_multiway_bet() {
        let c = |s| Card::parse(s).unwrap();
        let mut v = View {
            hole: [c("2d"), c("2c")],
            board: ["9h", "9s", "As"].map(c).to_vec(),
            bb: 20,
            pot: 350,
            bet: 0,
            stack: 865,
            effective: 865,
            late: false,
            ranges: vec![0.82, 0.4, 0.4, 0.4],
            legal: Legal {
                check: true,
                fold: true,
                raise: Some((20, 865)),
                ..Default::default()
            },
        };
        let old = contextual(&v, 21, 512, ADAPTIVE, false, Default::default());
        let new = contextual(&v, 21, 512, GUARDED_ADAPTIVE, false, Default::default());
        assert_eq!(old.action, Action::Raise(210));
        assert_eq!(old.reason, "trained_response");
        assert_eq!(new.action, Action::Check);
        assert_eq!(new.equity, old.equity);
        assert_eq!(new.reason, "guarded_multiway_check");
        assert!(!new.spends_response());
        for profile in 23..=MAX_LAB_PROFILE {
            assert_eq!(
                contextual(&v, 21, 512, profile, false, Default::default()).action,
                Action::Check
            );
        }
        v.ranges.truncate(1);
        assert!(!veto_multiway_bluff(&v, &new, old.action, false));
    }

    #[test]
    fn lab_early_steal_guard_preserves_late_steals_and_value_raises() {
        let c = |s| Card::parse(s).unwrap();
        let mut v = View {
            hole: [c("7h"), c("2s")],
            board: vec![],
            bb: 20,
            pot: 30,
            bet: 0,
            stack: 1000,
            effective: 1000,
            late: false,
            ranges: vec![0.; 4],
            legal: Legal {
                fold: true,
                call: Some(20),
                raise: Some((40, 1000)),
                ..Default::default()
            },
        };
        let d = |v: &View, p| contextual(v, 3, 64, p, false, Default::default());
        assert_eq!(d(&v, 22).reason, "trained_response");
        assert_eq!(d(&v, 23).action, Action::Fold);
        assert!(!d(&v, 23).spends_response());
        v.late = true;
        assert_eq!(d(&v, 23).action, d(&v, 22).action);
        v.late = false;
        v.hole = [c("Ah"), c("As")];
        assert!(matches!(d(&v, 23).action, Action::Raise(_)));
        assert_eq!(d(&v, 23).action, d(&v, 22).action);
        v.hole = [c("Ah"), c("Ks")];
        assert_eq!(d(&v, 22).action, Action::Call);
        assert_eq!(d(&v, 26).action, Action::Raise(100));
        assert!(!d(&v, 26).spends_response());
    }
    #[test]
    fn adaptation_requires_an_open_and_does_not_buy_a_second_override() {
        use crate::defense::Pressure;
        let c = |s| Card::parse(s).unwrap();
        let mut v = View {
            hole: [c("7c"), c("2d")],
            board: vec![],
            bb: 20,
            pot: 30,
            bet: 0,
            stack: 1000,
            effective: 1000,
            late: true,
            ranges: vec![0.; 3],
            legal: Legal {
                fold: true,
                call: Some(20),
                raise: Some((40, 1000)),
                ..Default::default()
            },
        };
        let h = Pressure {
            supported: true,
            fold_through: 0.25,
            reraise: 0.35,
            ..Default::default()
        };
        let base = with_profile(&v, 9, 64, 20);
        assert_eq!(
            contextual(&v, 9, 64, 21, false, Pressure::default()).action,
            base.action
        );
        let spent = contextual(&v, 9, 64, 21, true, h);
        assert_eq!(spent.action, with_profile(&v, 9, 64, 17).action);
        assert!(!spent.spends_response());
        let (_, opts) = steal_options(&v, Action::Raise(60), h).unwrap();
        assert_eq!(opts[3], Some(Action::Raise(180)));
        v.effective = 200;
        assert!(
            steal_options(&v, Action::Raise(60), h).unwrap().1[2..]
                .iter()
                .all(Option::is_none)
        );
        v.bet = 20;
        assert!(steal_options(&v, Action::Raise(60), h).is_none());
        v.bet = 0;
        v.hole = [c("Ac"), c("Kd")];
        assert!(steal_options(&v, Action::Raise(60), h).is_none());
        assert_eq!(
            contextual(&v, 9, 64, 21, false, h).action,
            with_profile(&v, 9, 64, 20).action
        );
    }
    #[test]
    fn learned_steal_spends_only_one_response_and_has_an_exposure_cap() {
        let c = |s| Card::parse(s).unwrap();
        let mut v = View {
            hole: [c("7c"), c("2d")],
            board: vec![],
            bb: 20,
            pot: 100,
            bet: 0,
            stack: 2000,
            effective: 2000,
            late: false,
            ranges: vec![0.82, 0.0, 0.0],
            legal: Legal {
                fold: true,
                call: Some(40),
                raise: Some((80, 2000)),
                ..Default::default()
            },
        };
        let first = with_profile(&v, 31, 64, budgeted_profile(20, false));
        assert_eq!(first.reason, "trained_response");
        assert!(matches!(first.action,Action::Raise(n) if n<=240));
        assert_eq!(
            with_profile(&v, 31, 64, budgeted_profile(20, true)).action,
            Action::Fold
        );
        v.legal.call = Some(220);
        v.pot = 340;
        v.legal.raise = Some((440, 2000));
        assert_eq!(with_profile(&v, 31, 64, 20).action, Action::Fold);
    }
    #[test]
    fn unbeatable_board_calls_instead_of_folding_or_raising() {
        let c = |s| Card::parse(s).unwrap();
        let v = View {
            hole: [c("2d"), c("3d")],
            board: ["Ah", "Kh", "Qh", "Jh", "Th"].map(c).to_vec(),
            bb: 20,
            pot: 160,
            bet: 0,
            stack: 100,
            effective: 100,
            late: false,
            ranges: vec![0.82; 5],
            legal: Legal {
                fold: true,
                call: Some(100),
                ..Default::default()
            },
        };
        let d = decide(&v, 10, 128);
        assert_eq!(d.action, Action::Call);
        assert_eq!(d.reason, "locked_split");
    }
    #[test]
    fn short_all_in_is_a_call_not_a_raise() {
        let c = |s| Card::parse(s).unwrap();
        let v = View {
            hole: [c("Ac"), c("Ad")],
            board: vec![],
            bb: 20,
            pot: 90,
            bet: 0,
            stack: 13,
            effective: 13,
            late: false,
            ranges: vec![0.82],
            legal: Legal {
                fold: true,
                call: Some(13),
                ..Default::default()
            },
        };
        assert_eq!(decide(&v, 2, 64).action, Action::Call);
    }
}
