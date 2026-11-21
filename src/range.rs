//! 169-hand canonical preflop valuation and 1,326-combo Bayesian opponent range tracking.

use crate::cards::{Card, evaluate};
use crate::hud::OppHud;
use std::sync::OnceLock;

/// Exact table position in 2–6 max.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pos {
    Utg,
    Hj,
    Co,
    Btn,
    Sb,
    Bb,
}

impl Pos {
    /// Map seat index to exact table position given `n` active seats and button seat.
    /// `active_order` is the clockwise list of active seat indices starting from button.
    pub fn from_offset_from_btn(offset: usize, n: usize) -> Self {
        if n <= 2 {
            return if offset == 0 { Pos::Btn } else { Pos::Bb };
        }
        match offset {
            0 => Pos::Btn,
            1 => Pos::Sb,
            2 => Pos::Bb,
            _ => {
                // Remaining positions before BTN: offset n-1 is CO, n-2 is HJ, n-3 is UTG.
                let from_btn_ccw = n - offset;
                match from_btn_ccw {
                    1 => Pos::Co,
                    2 => Pos::Hj,
                    _ => Pos::Utg,
                }
            }
        }
    }

    pub fn is_late(self) -> bool {
        matches!(self, Pos::Co | Pos::Btn)
    }

    pub fn is_blind(self) -> bool {
        matches!(self, Pos::Sb | Pos::Bb)
    }

    /// Standard 6-max RFI open percentile cutoff (fraction of 169 hands opened, 0% rake).
    pub fn rfi_width(self) -> f64 {
        match self {
            Pos::Utg => 0.18,
            Pos::Hj => 0.25,
            Pos::Co => 0.34,
            Pos::Btn => 0.50,
            Pos::Sb => 0.44,
            Pos::Bb => 0.40, // BB vs limps
        }
    }
}

/// Preflop action summary for one opponent in the current hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PreflopLine {
    #[default]
    Unacted,
    BbCheck,
    Limp,
    ColdCall,
    OpenRaise,
    Call3Bet,
    ThreeBet,
    FourBetPlus,
}

/// Canonical 169-hand index:
/// - pairs: 0..13 (22=0 .. AA=12)
/// - suited: 13..91
/// - offsuit: 91..169
pub fn canon_index(c1: Card, c2: Card) -> usize {
    let r1 = (c1.rank() - 2) as usize;
    let r2 = (c2.rank() - 2) as usize;
    let hi = r1.max(r2);
    let lo = r1.min(r2);
    if hi == lo {
        hi
    } else {
        let pair_idx = hi * (hi - 1) / 2 + lo; // 0..78
        if c1.suit() == c2.suit() {
            13 + pair_idx
        } else {
            91 + pair_idx
        }
    }
}

/// 6-max 0%-rake deep-stack playability score in [0.0, 1.0] for any two hole cards.
/// Properly ranks pairs, suited aces, suited broadways, suited connectors above dominated offsuit trash.
pub fn playability_score(c1: Card, c2: Card) -> f64 {
    let r1 = c1.rank() as f64; // 2..=14
    let r2 = c2.rank() as f64;
    let hi = r1.max(r2);
    let lo = r1.min(r2);
    let suited = c1.suit() == c2.suit();
    let gap = hi - lo;

    if hi == lo {
        // 22 -> 0.65, 55 -> 0.73, 88 -> 0.82, JJ -> 0.93, QQ -> 0.96, KK -> 0.985, AA -> 1.00
        return match hi as u8 {
            14 => 1.000,
            13 => 0.985,
            12 => 0.965,
            11 => 0.940,
            10 => 0.905,
            9 => 0.860,
            8 => 0.820,
            7 => 0.785,
            6 => 0.750,
            5 => 0.720,
            4 => 0.690,
            3 => 0.665,
            _ => 0.640,
        };
    }

    // Base high-card equity value
    let mut score = 0.030 * hi + 0.022 * lo - 0.14;

    // Premium broadway bonuses
    if hi == 14.0 && lo == 13.0 {
        score = if suited { 0.975 } else { 0.945 };
    } else if hi == 14.0 && lo == 12.0 {
        score = if suited { 0.930 } else { 0.880 };
    } else if hi == 14.0 && lo == 11.0 {
        score = if suited { 0.895 } else { 0.830 };
    } else if hi == 14.0 && lo == 10.0 {
        score = if suited { 0.865 } else { 0.780 };
    } else if hi == 13.0 && lo == 12.0 {
        score = if suited { 0.890 } else { 0.825 };
    } else if hi == 13.0 && lo == 11.0 {
        score = if suited { 0.855 } else { 0.770 };
    } else if hi == 13.0 && lo == 10.0 {
        score = if suited { 0.820 } else { 0.725 };
    } else if hi == 12.0 && lo == 11.0 {
        score = if suited { 0.840 } else { 0.745 };
    } else if hi == 12.0 && lo == 10.0 {
        score = if suited { 0.800 } else { 0.700 };
    } else if hi == 11.0 && lo == 10.0 {
        score = if suited { 0.815 } else { 0.690 };
    } else {
        if suited {
            score += 0.165;
            // Suited Aces (wheel bonus for A5s-A2s)
            if hi == 14.0 {
                score += 0.125;
                if lo <= 5.0 {
                    score += 0.045;
                }
            }
            // Suited Kings
            else if hi == 13.0 {
                score += 0.065;
            }
            // Suited connectors & 1-gappers (T9s, 98s, 87s, 76s, 65s, 54s)
            if gap == 1.0 && lo >= 4.0 {
                score += 0.155;
            } else if gap == 2.0 && lo >= 5.0 {
                score += 0.095;
            } else if gap == 3.0 && lo >= 6.0 {
                score += 0.040;
            }
        } else {
            // Offsuit hands: heavily penalize dominated offsuit rags & wide gaps
            if hi == 14.0 {
                score += 0.065;
                if lo >= 7.0 {
                    score += 0.045;
                }
            } else if gap == 1.0 && lo >= 8.0 {
                score += 0.075;
            } else if gap == 1.0 && lo >= 5.0 {
                score += 0.035;
            } else if gap >= 4.0 && hi <= 13.0 {
                score -= 0.085;
            }
        }
    }

    score.clamp(0.01, 0.999)
}

/// Raw all-in equity strength (for short-stack shove / call / 4-bet jams).
/// Favors pairs and high cards over suited connectors.
pub fn allin_strength(c1: Card, c2: Card) -> f64 {
    let hi = c1.rank().max(c2.rank()) as f64;
    let lo = c1.rank().min(c2.rank()) as f64;
    let suited = c1.suit() == c2.suit();
    if hi == lo {
        return (0.56 + 0.036 * (hi - 2.0)).min(1.0);
    }
    let mut s = 0.038 * hi + 0.024 * lo - 0.14;
    if suited {
        s += 0.065;
    }
    if hi == 14.0 {
        s += 0.10;
    }
    if hi - lo == 1.0 && lo >= 7.0 {
        s += 0.03;
    }
    s.clamp(0.01, 0.97)
}

pub struct ComboTables {
    pub cards: [(Card, Card); 1326],
    pub masks: [u64; 1326],
    pub canon: [u8; 1326],
    /// Rank percentile of each combo in [0.0, 1.0] (1.0 = AA, 0.0 = 72o), sorted by playability.
    pub percentile: [f32; 1326],
    /// Raw playability score.
    pub playability: [f32; 1326],
}

pub fn combo_tables() -> &'static ComboTables {
    static TABLES: OnceLock<ComboTables> = OnceLock::new();
    TABLES.get_or_init(|| {
        let deck = crate::cards::deck();
        let mut cards = [(deck[0], deck[1]); 1326];
        let mut masks = [0u64; 1326];
        let mut canon = [0u8; 1326];
        let mut playability = [0.0f32; 1326];
        let mut idx = 0;
        for i in 0..52 {
            for j in (i + 1)..52 {
                cards[idx] = (deck[i], deck[j]);
                masks[idx] = deck[i].bit() | deck[j].bit();
                canon[idx] = canon_index(deck[i], deck[j]) as u8;
                playability[idx] = playability_score(deck[i], deck[j]) as f32;
                idx += 1;
            }
        }
        assert_eq!(idx, 1326);
        // Compute exact percentile rank across all 1326 combos
        let mut order: Vec<usize> = (0..1326).collect();
        order.sort_by(|&a, &b| playability[a].total_cmp(&playability[b]));
        let mut percentile = [0.0f32; 1326];
        for (rank, &ci) in order.iter().enumerate() {
            percentile[ci] = rank as f32 / 1325.0;
        }
        ComboTables {
            cards,
            masks,
            canon,
            percentile,
            playability,
        }
    })
}

/// Fast hand-strength bucket `0..=9` for a 2-card holding on a 3–5 card board.
/// Used both for Bayesian range updates and opponent response modeling.
#[inline]
pub fn postflop_bucket(h: [Card; 2], board: &[Card], board_mask: u64, board_info: &BoardFast) -> u8 {
    let h_mask = h[0].bit() | h[1].bit();
    let bits = board_mask | h_mask;
    let score = evaluate(bits);
    let cat = (score >> 24) as u8;

    let r0 = h[0].rank();
    let r1 = h[1].rank();
    let hi = r0.max(r1);
    let lo = r0.min(r1);
    let is_pair = r0 == r1;

    // Check flush/straight draws on flop/turn
    let mut flush_draw = false;
    let mut straight_draw = false;
    if board.len() < 5 && cat < 4 {
        for s in 0..4u8 {
            let sm = ((bits >> (13 * s)) & 8191) as u16;
            if sm.count_ones() == 4 && (h[0].suit() == s || h[1].suit() == s) {
                flush_draw = true;
                break;
            }
        }
        // Quick OESD check on 13-bit rank mask
        let rm = (((bits) | (bits >> 13) | (bits >> 26) | (bits >> 39)) & 8191) as u16;
        let hm = (1u16 << (r0 - 2)) | (1u16 << (r1 - 2));
        // 4-in-a-row window involving at least one hole card
        let four_seq = rm & (rm << 1) & (rm << 2) & (rm << 3);
        if four_seq != 0 && (four_seq & (hm | (hm << 1) | (hm << 2) | (hm << 3))) != 0 {
            straight_draw = true;
        }
    }

    // Classify made hand
    if cat >= 6 {
        return 9; // Full house, quads, straight flush
    }
    if cat == 5 {
        // Flush: check if hole card participates
        let mut flush_suit = 4u8;
        for s in 0..4u8 {
            if (((bits >> (13 * s)) & 8191) as u16).count_ones() >= 5 {
                flush_suit = s;
                break;
            }
        }
        let max_h = if h[0].suit() == flush_suit && h[1].suit() == flush_suit {
            hi
        } else if h[0].suit() == flush_suit {
            r0
        } else if h[1].suit() == flush_suit {
            r1
        } else {
            0
        };
        if max_h == 0 {
            return 2; // board flush only
        }
        if board_info.max_suit_count >= 4 {
            // 4-flush board: only A/K of the flush suit are true monsters!
            return match max_h {
                14 => 9,
                13 => 8,
                11..=12 => 6,
                _ => 4,
            };
        }
        return if max_h >= 13 { 9 } else { 8 };
    }
    if cat == 4 {
        // Straight: check if board already has a straight
        if board.len() == 5 && (evaluate(board_mask) >> 24) >= 4 && score == evaluate(board_mask) {
            return 2;
        }
        return 8;
    }
    if cat == 3 {
        // Trips or Set
        if board_info.has_trips {
            // Board has trips; hero only has high kicker
            return if hi == 14 { 4 } else { 2 };
        }
        if is_pair {
            return 8; // Pocket pair -> Set!
        }
        return 7; // Trips with one hole card
    }
    if cat == 2 {
        // Two pair: distinguish true two pair from board-paired + one pair!
        if board_info.num_pairs == 0 {
            return 7; // Both hole cards paired the board!
        }
        if board_info.num_pairs >= 2 {
            // Double-paired board: check if hero improved over board
            if is_pair && r0 > board_info.low_pair_rank {
                return 6;
            }
            return if hi == 14 { 3 } else { 1 };
        }
        // Single pair on board + hero has a pair
        if is_pair {
            return if r0 > board_info.top_rank {
                6 // Overpair on paired board
            } else if r0 > board_info.second_rank {
                5
            } else {
                3
            };
        }
        // Hero paired an unpaired board card
        if r0 == board_info.top_rank || r1 == board_info.top_rank {
            let kicker = if r0 == board_info.top_rank { r1 } else { r0 };
            return if kicker >= 11 { 6 } else { 5 };
        }
        if board_info.rank_counts[r0 as usize] == 1 || board_info.rank_counts[r1 as usize] == 1 {
            return 3;
        }
        return 1;
    }
    if cat == 1 {
        if board_info.num_pairs > 0 {
            // Board pair only!
            if flush_draw || straight_draw {
                return 4;
            }
            return if hi == 14 && lo >= 10 { 1 } else { 0 };
        }
        if is_pair {
            if r0 > board_info.top_rank {
                return 6; // Overpair
            }
            if r0 > board_info.second_rank {
                return 5; // Second-high pocket pair (e.g. QQ on K72)
            }
            return 3; // Underpair / middle pocket pair
        }
        if r0 == board_info.top_rank || r1 == board_info.top_rank {
            let kicker = if r0 == board_info.top_rank { r1 } else { r0 };
            return if kicker >= 11 || (board_info.top_rank == 14 && kicker >= 10) {
                6 // TPGK / TPTK
            } else if kicker >= 8 {
                5 // TPMK
            } else {
                4 // TPWK (or weak top pair)
            };
        }
        if r0 == board_info.second_rank || r1 == board_info.second_rank {
            return if flush_draw || straight_draw { 4 } else { 3 }; // Middle pair
        }
        // Bottom pair
        return if flush_draw || straight_draw { 4 } else { 2 };
    }

    // High card
    if flush_draw && straight_draw {
        return 5; // Monster combo draw!
    }
    if flush_draw || straight_draw {
        return 4; // Strong draw
    }
    if hi == 14 && lo >= 11 {
        return 1; // Strong overcards / Ace-high
    }
    0 // Pure air
}

#[derive(Clone, Copy, Debug)]
pub struct BoardFast {
    pub rank_counts: [u8; 15],
    pub top_rank: u8,
    pub second_rank: u8,
    pub num_pairs: u8,
    pub low_pair_rank: u8,
    pub has_trips: bool,
    pub max_suit_count: u8,
    pub flush_suit: Option<u8>,
    pub wetness: f32,
}

impl BoardFast {
    pub fn analyze(board: &[Card]) -> Self {
        let mut rank_counts = [0u8; 15];
        let mut suit_counts = [0u8; 4];
        let mut top_rank = 0u8;
        let mut second_rank = 0u8;
        for &c in board {
            let r = c.rank();
            rank_counts[r as usize] += 1;
            suit_counts[c.suit() as usize] += 1;
            if r > top_rank {
                second_rank = top_rank;
                top_rank = r;
            } else if r < top_rank && r > second_rank {
                second_rank = r;
            }
        }
        let mut num_pairs = 0u8;
        let mut low_pair_rank = 15u8;
        let mut has_trips = false;
        for r in 2..=14u8 {
            if rank_counts[r as usize] == 2 {
                num_pairs += 1;
                low_pair_rank = low_pair_rank.min(r);
            } else if rank_counts[r as usize] >= 3 {
                has_trips = true;
            }
        }
        let mut max_suit_count = 0u8;
        let mut flush_suit = None;
        for s in 0..4u8 {
            if suit_counts[s as usize] > max_suit_count {
                max_suit_count = suit_counts[s as usize];
                if max_suit_count >= 3 {
                    flush_suit = Some(s);
                }
            }
        }
        // Connectedness score
        let mut ranks: Vec<u8> = board.iter().map(|c| c.rank()).collect();
        ranks.sort_unstable();
        ranks.dedup();
        let mut close_pairs = 0u32;
        for w in ranks.windows(2) {
            if w[1] - w[0] <= 2 {
                close_pairs += 1;
            }
        }
        let mut wetness = 0.0f32;
        if max_suit_count >= 4 {
            wetness += 0.55;
        } else if max_suit_count == 3 {
            wetness += 0.38;
        } else if max_suit_count == 2 && board.len() < 5 {
            wetness += 0.18;
        }
        wetness += (close_pairs as f32) * 0.18;
        if num_pairs > 0 || has_trips {
            wetness -= 0.12;
        }
        Self {
            rank_counts,
            top_rank,
            second_rank,
            num_pairs,
            low_pair_rank,
            has_trips,
            max_suit_count,
            flush_suit,
            wetness: wetness.clamp(0.0, 1.0),
        }
    }
}

/// Explicit 1,326-combo probability distribution for one opponent.
#[derive(Clone, Debug)]
pub struct ComboRange {
    pub weights: [f32; 1326],
}

impl ComboRange {
    /// Build a preflop range from opponent's position, preflop line, and Bayesian HUD stats.
    pub fn from_preflop(pos: Pos, line: PreflopLine, hud: &OppHud, dead_mask: u64) -> Self {
        let t = combo_tables();
        let mut w = [0.0f32; 1326];

        // Determine target [lo_pct, hi_pct] band of the 1326-combo ordering.
        // Adjust widths smoothly by opponent HUD VPIP/PFR/3-bet!
        let vpip_scale = (hud.vpip / 0.32).clamp(0.55, 2.0);
        let pfr_scale = (hud.pfr / 0.17).clamp(0.55, 2.2);
        let tb_scale = (hud.three_bet / 0.04).clamp(0.45, 2.5);

        let (lo, hi, polar_bluff) = match line {
            PreflopLine::FourBetPlus => {
                let width = (0.045 * tb_scale).clamp(0.025, 0.12);
                (1.0 - width, 1.0, 0.02)
            }
            PreflopLine::ThreeBet => {
                let width = (0.085 * tb_scale).clamp(0.04, 0.22);
                (1.0 - width, 1.0, 0.04 * hud.aggression())
            }
            PreflopLine::Call3Bet => {
                let top_cap = 0.96; // usually 4-bets AA/KK
                let width = (0.16 * vpip_scale).clamp(0.08, 0.35);
                ((top_cap - width).max(0.40), top_cap, 0.02)
            }
            PreflopLine::OpenRaise => {
                let base = pos.rfi_width();
                let width = (base * pfr_scale).clamp(0.10, 0.72);
                (1.0 - width, 1.0, 0.03)
            }
            PreflopLine::ColdCall => {
                // Capped at top (most 3-bet top ~5-8%, stations flat wider)
                let top_cap = if hud.three_bet < 0.03 { 0.99 } else { 0.94 };
                let base_width = if pos == Pos::Bb { 0.44 } else { 0.24 };
                let width = (base_width * vpip_scale).clamp(0.12, 0.78);
                ((top_cap - width).max(0.05), top_cap, 0.04)
            }
            PreflopLine::Limp => {
                let width = (0.42 * vpip_scale).clamp(0.20, 0.80);
                ((0.92 - width).max(0.05), 0.93, 0.05)
            }
            PreflopLine::BbCheck => {
                // Unraised pot BB check: everything that didn't raise!
                (0.0, 0.90, 0.05)
            }
            PreflopLine::Unacted => (0.0, 1.0, 0.0),
        };

        let lo = lo as f32;
        let hi = hi as f32;
        let polar = polar_bluff as f32;

        let mut sum = 0.0f32;
        for i in 0..1326 {
            if (t.masks[i] & dead_mask) != 0 {
                continue;
            }
            let p = t.percentile[i];
            // Soft sigmoid edges so range is realistic and never brittle
            let in_band = if p >= lo && p <= hi {
                1.0f32
            } else if p < lo {
                let d = (lo - p) / 0.07;
                (-d * d).exp().max(polar)
            } else {
                // Above cap (traps / slowplays)
                let d = (p - hi) / 0.05;
                (-d * d).exp().max(0.18)
            };
            w[i] = in_band.max(1e-4);
            sum += w[i];
        }

        if sum > 0.0 {
            let inv = 1.0 / sum;
            for x in &mut w {
                *x *= inv;
            }
        }
        Self { weights: w }
    }

    /// Zero out newly dealt board cards or hero hole cards and renormalize.
    pub fn apply_dead_mask(&mut self, dead_mask: u64) {
        let t = combo_tables();
        let mut sum = 0.0f32;
        for i in 0..1326 {
            if (t.masks[i] & dead_mask) != 0 {
                self.weights[i] = 0.0;
            } else {
                sum += self.weights[i];
            }
        }
        if sum > 1e-9 {
            let inv = 1.0 / sum;
            for x in &mut self.weights {
                *x *= inv;
            }
        }
    }

    /// Bayesian update given an observed postflop action by this opponent.
    /// `action_kind`: 0=check, 1=call, 2=bet, 3=raise
    /// `is_multiway`: true when 3+ players were active at the time of the action
    /// `is_donk`: true when opponent led OOP into the preflop aggressor on the flop
    #[allow(clippy::too_many_arguments)]
    pub fn update_postflop(
        &mut self,
        board: &[Card],
        board_mask: u64,
        board_info: &BoardFast,
        action_kind: u8,
        bet_to_pot: f32,
        street: usize,
        hud: &OppHud,
        is_multiway: bool,
        is_donk: bool,
    ) {
        let t = combo_tables();
        let sticky = hud.stickiness() as f32;
        let aggr = hud.aggression() as f32;
        let is_river = street == 3;
        let is_turn = street == 2;
        // Real OpenPoker opponents bet only ~27% when checked to; multiway leads, OOP donk-bets,
        // and turn/river barrels from non-maniacs are heavily value-weighted!
        let mw_bluff_mult = if is_multiway { 0.48 } else { 1.0 };
        let donk_bluff_mult = if is_donk { 0.55 } else { 1.0 };
        let street_bluff_mult = if is_river {
            0.52
        } else if is_turn {
            0.72
        } else {
            1.0
        };
        let passive_bet_mult = (hud.agg_freq as f32 / 0.30).clamp(0.45, 1.55);

        // Precompute likelihood multiplier per bucket 0..=9
        let mult: [f32; 10] = match action_kind {
            0 => {
                // Check: caps nutted hands slightly, keeps air & showdown pairs
                let trap = 0.25 + 0.25 * (1.0 - aggr);
                [0.85, 0.80, 0.78, 0.75, 0.55, 0.52, 0.40, trap, trap * 0.85, trap * 0.8]
            }
            1 => {
                // Call facing a bet of size `bet_to_pot` (true B / pot_before_bet)
                let size_pen = (bet_to_pot / 0.70).clamp(0.40, 2.60);
                let air_call = (0.02 + 0.22 * sticky * sticky) / size_pen;
                let weak_call = (0.14 + 0.45 * sticky) / size_pen.sqrt();
                let mid_call = (0.42 + 0.42 * sticky).min(0.95);
                let draw_call = if is_river { air_call } else { 0.84 };
                [
                    air_call.clamp(0.01, 0.50),
                    (air_call * 1.7).clamp(0.02, 0.60),
                    weak_call.clamp(0.06, 0.85),
                    mid_call,
                    draw_call,
                    0.88,
                    0.92,
                    0.65,
                    0.50,
                    0.45,
                ]
            }
            2 => {
                // Bet (lead / c-bet / probe / barrel)
                let bluff_base = (0.04 + 0.36 * aggr)
                    * passive_bet_mult
                    * mw_bluff_mult
                    * donk_bluff_mult
                    * street_bluff_mult;
                let weak_val: f32 = if is_multiway || is_donk || is_turn || is_river {
                    0.55
                } else {
                    1.0
                };
                if bet_to_pot <= 0.45 {
                    // Small bet: merged value + some bluffs
                    [
                        (bluff_base * 1.05).clamp(0.02, 0.42),
                        (bluff_base * 1.20).clamp(0.03, 0.48),
                        (0.28 * weak_val).max(0.08),
                        (0.42 * weak_val).max(0.14),
                        if is_river { bluff_base.clamp(0.02, 0.35) } else { 0.72 },
                        0.82,
                        0.92,
                        0.88,
                        0.84,
                        0.76,
                    ]
                } else if bet_to_pot <= 0.90 {
                    // Medium-large bet: value-heavy + semi-bluffs
                    [
                        (bluff_base * 0.55).clamp(0.015, 0.30),
                        (bluff_base * 0.68).clamp(0.02, 0.34),
                        (0.12 * weak_val).max(0.04),
                        (0.22 * weak_val).max(0.08),
                        if is_river { (bluff_base * 0.60).clamp(0.015, 0.25) } else { 0.78 },
                        0.72,
                        0.94,
                        0.98,
                        1.00,
                        0.96,
                    ]
                } else {
                    // Overbet (> 90% pot): strongly polarized
                    [
                        (bluff_base * 0.40).clamp(0.015, 0.26),
                        (bluff_base * 0.45).clamp(0.015, 0.28),
                        0.04,
                        0.07,
                        if is_river { (bluff_base * 0.45).clamp(0.015, 0.22) } else { 0.62 },
                        0.22,
                        0.55,
                        0.92,
                        1.00,
                        1.00,
                    ]
                }
            }
            _ => {
                // Raise / Check-raise: distinguish frequent small-raisers (Kairo/minhcasa, r2b > 18%)
                // from normal/tight players whose raises are overwhelmingly two-pair+ / sets!
                let high_r2b = hud.raise_vs_bet > 0.18 && bet_to_pot <= 0.60;
                let r_bluff = (0.015 + 0.30 * aggr * aggr) * mw_bluff_mult * street_bluff_mult;
                if high_r2b {
                    [
                        (r_bluff * 1.4).clamp(0.02, 0.25),
                        (r_bluff * 1.6).clamp(0.02, 0.28),
                        0.08,
                        0.16,
                        if is_river { r_bluff } else { 0.55 + 0.25 * aggr },
                        0.45,
                        0.72,
                        0.95,
                        1.00,
                        1.00,
                    ]
                } else {
                    [
                        r_bluff,
                        r_bluff * 1.2,
                        0.02 + 0.08 * aggr,
                        0.04 + 0.10 * aggr,
                        if is_river { r_bluff } else { 0.32 + 0.36 * aggr },
                        0.10 + 0.18 * aggr,
                        0.32 + 0.28 * aggr,
                        0.92,
                        1.00,
                        1.00,
                    ]
                }
            }
        };

        let mut sum = 0.0f32;
        for i in 0..1326 {
            let wi = self.weights[i];
            if wi <= 0.0 {
                continue;
            }
            let (c1, c2) = t.cards[i];
            let b = postflop_bucket([c1, c2], board, board_mask, board_info) as usize;
            let nw = (wi * mult[b]).max(1e-7);
            self.weights[i] = nw;
            sum += nw;
        }
        if sum > 1e-12 {
            let inv = 1.0 / sum;
            for x in &mut self.weights {
                *x *= inv;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playability_ranks_suited_connectors_over_offsuit_trash() {
        let c = |s| Card::parse(s).unwrap();
        assert!(playability_score(c("Ah"), c("As")) > playability_score(c("Kh"), c("Ks")));
        assert!(playability_score(c("9h"), c("8h")) > playability_score(c("Kd"), c("5c")));
        assert!(playability_score(c("Ah"), c("5h")) > playability_score(c("Qc"), c("4d")));
        assert!(playability_score(c("7h"), c("7d")) > playability_score(c("Jc"), c("6d")));
    }

    #[test]
    fn range_normalizes_and_respects_blockers() {
        let c = |s| Card::parse(s).unwrap();
        let dead = c("Ah").bit() | c("Ad").bit();
        let r = ComboRange::from_preflop(Pos::Utg, PreflopLine::OpenRaise, &OppHud::pool_prior(), dead);
        let sum: f32 = r.weights.iter().sum();
        assert!((sum - 1.0).abs() < 1e-4);
        let t = combo_tables();
        for i in 0..1326 {
            if (t.masks[i] & dead) != 0 {
                assert_eq!(r.weights[i], 0.0);
            }
        }
    }
}
