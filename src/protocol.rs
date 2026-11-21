//! Player-visible state only. A public snapshot updates facts but never grants a turn.
use crate::{
    cards::{Card, mask},
    hud::HudTracker,
    policy::{Action, Chips, Legal, View},
    range::{BoardFast, ComboRange, Pos, PreflopLine},
    v2::{V2Context, V2Opp},
};
use serde_json::{Value, json};

fn chip(v: &Value) -> Option<Chips> {
    v.as_u64().and_then(|x| x.try_into().ok())
}
fn seat(v: &Value) -> Option<usize> {
    v.as_u64().filter(|&x| x < 6).map(|x| x as usize)
}
fn cards(v: &Value) -> Option<Vec<Card>> {
    v.as_array()?
        .iter()
        .map(|x| Card::parse(x.as_str()?))
        .collect()
}
pub fn legal(v: &Value) -> Legal {
    let mut l = Legal::default();
    for a in v.as_array().into_iter().flatten() {
        match a["action"].as_str() {
            Some("check") => l.check = true,
            Some("fold") => l.fold = true,
            Some("call") => l.call = chip(&a["amount"]),
            Some("raise") => {
                l.raise = chip(&a["min"])
                    .zip(chip(&a["max"]))
                    .filter(|(lo, hi)| lo <= hi && *lo > 0)
            }
            Some("all_in") => l.jam = chip(&a["amount"]),
            _ => (),
        }
    }
    l
}
fn parse_street(s: &str) -> Option<usize> {
    match s {
        "preflop" => Some(0),
        "flop" => Some(1),
        "turn" => Some(2),
        "river" => Some(3),
        "showdown" => Some(4),
        _ => None,
    }
}
fn board_street(board: &[Card]) -> usize {
    match board.len() {
        3 => 1,
        4 => 2,
        5 => 3,
        _ => 0,
    }
}
#[derive(Default, Clone, Copy)]
struct Seat {
    active: bool,
    folded: bool,
    stack: Chips,
    bet: Chips,
    range: f64,
    start_stack: Option<Chips>,
}
#[derive(Default)]
pub struct State {
    pub hand: String,
    pub table: String,
    pub seq: u64,
    pub hero: Option<usize>,
    button: Option<usize>,
    bb: Chips,
    pot: Chips,
    street: usize,
    seats: [Seat; 6],
    names: [String; 6],
    defense: crate::defense::Defense,
    hud: HudTracker,
    pf_raises: u8,
    limpers: u8,
    pfa_seat: Option<usize>,
    street_raises: u8,
    street_aggressor: Option<usize>,
    checked_street: [bool; 6],
    pf_line: [PreflopLine; 6],
    post_actions: [Vec<(usize, u8, f32)>; 6],
    hole: Option<[Card; 2]>,
    board: Option<Vec<Card>>,
}
impl State {
    pub fn take_hud(&mut self) -> HudTracker {
        std::mem::take(&mut self.hud)
    }
    pub fn set_hud(&mut self, hud: HudTracker) {
        self.hud = hud;
    }
    pub fn table(&mut self, id: &str) {
        if !id.is_empty() && id != self.table {
            let hud = std::mem::take(&mut self.hud);
            *self = Self {
                table: id.to_owned(),
                hud,
                ..Default::default()
            };
        }
    }
    fn hand(&mut self, id: &str) {
        if self.hand != id {
            self.defense.next_hand();
            self.hud.new_hand();
            self.hand = id.into();
            self.hole = None;
            self.board = Some(vec![]);
            self.pot = 0;
            self.street = 0;
            self.pf_raises = 0;
            self.limpers = 0;
            self.pfa_seat = None;
            self.street_raises = 0;
            self.street_aggressor = None;
            self.checked_street = [false; 6];
            self.pf_line = [PreflopLine::Unacted; 6];
            self.post_actions = Default::default();
            for s in &mut self.seats {
                s.bet = 0;
                s.folded = false;
                s.range = 0.0;
                s.start_stack = None;
            }
        }
    }
    fn advance_street(&mut self, next: usize, saw_flop_if_sd: bool) {
        if next > self.street {
            if self.street == 0 && (matches!(next, 1..=3) || (next == 4 && saw_flop_if_sd)) {
                self.hud.on_street_advance(1);
                self.defense.end_street();
            }
            self.street = next;
            if next <= 3 {
                self.street_raises = 0;
                self.street_aggressor = None;
                self.checked_street = [false; 6];
                for s in &mut self.seats {
                    s.bet = 0;
                }
            }
        }
    }
    fn board(&mut self, board: Option<Vec<Card>>) {
        if let Some(ref b) = board {
            if !b.is_empty() {
                self.defense.end_street();
            }
            self.advance_street(board_street(b), false);
        }
        self.board = board;
    }
    fn seats(&mut self, v: &Value, snapshot: bool) {
        let Some(arr) = v.as_array() else {
            return;
        };
        let mut present = [false; 6];
        for p in arr {
            if let Some(i) = seat(&p["seat"]) {
                present[i] = true;
                if let Some(nm) = p["name"].as_str().filter(|n| !n.is_empty()) {
                    self.names[i] = nm.to_owned();
                } else if snapshot {
                    self.names[i].clear();
                }
                let s = &mut self.seats[i];
                if snapshot || p.get("in_hand").is_some() {
                    s.active = p["in_hand"].as_bool().unwrap_or(false);
                } else if let Some(st) = p["status"].as_str() {
                    s.active = !matches!(st, "empty" | "sitting_out" | "busted" | "out");
                } else {
                    s.active = true;
                }
                if s.active && !self.names[i].is_empty() {
                    self.hud.register_seat(i, &self.names[i], true);
                }
                if let Some(x) = p["stack"].as_u64().and_then(|v| v.try_into().ok()) {
                    s.stack = x;
                }
                if let Some(x) = chip(&p["bet"]) {
                    s.bet = x;
                    // Preflop, all chips committed so far are in the street bet.
                    // A first snapshot on a later street cannot recover this ledger.
                    if snapshot
                        && self.board.as_ref().is_some_and(Vec::is_empty)
                        && s.active
                        && s.start_stack.is_none()
                    {
                        s.start_stack = s.stack.checked_add(x);
                    }
                }
                if let Some(x) = p["folded"].as_bool() {
                    s.folded = x;
                } else if p["status"].as_str() == Some("folded") {
                    s.folded = true;
                }
            }
        }
        if !arr.is_empty() {
            for i in 0..6 {
                if !present[i] {
                    self.seats[i].active = false;
                    self.seats[i].bet = 0;
                    self.seats[i].folded = false;
                    self.seats[i].start_stack = None;
                    if snapshot {
                        self.names[i].clear();
                    }
                }
            }
        }
    }
    pub fn observe(&mut self, m: &Value) {
        let kind = m["type"].as_str().unwrap_or("");
        if let Some(id) = m["hand_id"].as_str() {
            self.hand(id);
        }
        match kind {
            "table_joined" => {
                self.hero = seat(&m["seat"]);
                self.seats(&m["players"], false);
            }
            "hand_start" => {
                self.hero = seat(&m["seat"]).or(self.hero);
                self.button = seat(&m["dealer_seat"]);
                self.bb = chip(&m["blinds"]["big_blind"]).unwrap_or(20);
            }
            "hole_cards" => self.hole = cards(&m["cards"]).and_then(|c| c.try_into().ok()),
            "table_state" => {
                self.button = seat(&m["dealer_seat"]);
                self.bb = chip(&m["big_blind"]).unwrap_or(20);
                self.pot = chip(&m["pot"]).unwrap_or(0);
                self.board(cards(&m["board"]));
                if let Some(st) = m["street"].as_str().and_then(parse_street) {
                    self.advance_street(st, false);
                }
                self.seats(&m["seats"], true);
                let h = &m["hero"];
                self.hero = seat(&h["seat"]).or(self.hero);
                if let Some(c) = cards(&h["hole_cards"]) {
                    self.hole = c.try_into().ok();
                }
            }
            "your_turn" => {
                if let Some(p) = chip(&m["pot"]) {
                    self.pot = p;
                }
                self.board(cards(&m["community_cards"]));
                self.seats(&m["players"], false);
            }
            "player_action" => {
                if let Some(i) = seat(&m["seat"]) {
                    self.defense.action(m, self.hero, i);
                    self.seats[i].active = true;
                    if let Some(nm) = m["name"].as_str().filter(|n| !n.is_empty()) {
                        self.names[i] = nm.to_owned();
                    }
                    let act_str = m["action"].as_str().unwrap_or("");
                    let next_street = m["street"].as_str().and_then(parse_street);
                    let act_street = if next_street.is_some() {
                        self.street.min(3)
                    } else {
                        self.board.as_deref().map_or(0, board_street)
                    };
                    let owing = chip(&m["to_call_before"]).unwrap_or(0);
                    let raw_delta = chip(&m["contribution_delta"]).unwrap_or(0);
                    let delta = if act_str == "all_in" && raw_delta == 0 {
                        chip(&m["stack_before"]).unwrap_or(self.seats[i].stack)
                    } else {
                        raw_delta
                    };
                    let pot_b = chip(&m["pot_before"]).unwrap_or(self.pot);
                    let is_r = act_str == "raise" || (act_str == "all_in" && delta > owing);
                    let nm = m["name"]
                        .as_str()
                        .filter(|n| !n.is_empty())
                        .unwrap_or(self.names[i].as_str());
                    self.hud
                        .on_action(i, nm, act_street, act_str, owing, pot_b, is_r);
                    if act_street == 0 {
                        if is_r {
                            self.pf_raises = self.pf_raises.saturating_add(1);
                            self.pfa_seat = Some(i);
                            self.street_raises = self.street_raises.saturating_add(1);
                            self.street_aggressor = Some(i);
                            self.pf_line[i] = match self.pf_raises {
                                1 => PreflopLine::OpenRaise,
                                2 => PreflopLine::ThreeBet,
                                _ => PreflopLine::FourBetPlus,
                            };
                        } else if matches!(act_str, "call" | "all_in") {
                            if self.pf_raises == 0 {
                                self.limpers = self.limpers.saturating_add(1);
                                self.pf_line[i] = PreflopLine::Limp;
                            } else if self.pf_raises == 1 {
                                self.pf_line[i] = PreflopLine::ColdCall;
                            } else {
                                self.pf_line[i] = PreflopLine::Call3Bet;
                            }
                        } else if act_str == "check" {
                            self.pf_line[i] = PreflopLine::BbCheck;
                        }
                    } else {
                        let pot_f = pot_b.max(1) as f32;
                        match act_str {
                            "check" => {
                                self.checked_street[i] = true;
                                self.post_actions[i].push((act_street, 0, 0.0));
                            }
                            "call" => {
                                self.post_actions[i].push((act_street, 1, owing as f32 / pot_f));
                            }
                            "raise" => {
                                let k = if self.street_raises == 0 { 2 } else { 3 };
                                self.street_raises = self.street_raises.saturating_add(1);
                                self.street_aggressor = Some(i);
                                self.post_actions[i].push((act_street, k, delta as f32 / pot_f));
                            }
                            "all_in" => {
                                if is_r {
                                    let k = if self.street_raises == 0 { 2 } else { 3 };
                                    self.street_raises = self.street_raises.saturating_add(1);
                                    self.street_aggressor = Some(i);
                                    self.post_actions[i].push((act_street, k, delta as f32 / pot_f));
                                } else {
                                    self.post_actions[i].push((act_street, 1, delta as f32 / pot_f));
                                }
                            }
                            _ => (),
                        }
                    }
                    let s = &mut self.seats[i];
                    match act_str {
                        "fold" => s.folded = true,
                        "raise" | "all_in" => s.range = 0.82,
                        "call" => s.range = s.range.max(0.4),
                        _ => (),
                    };
                    if let Some(c) = chip(&m["stack"]) {
                        s.stack = c;
                    }
                    if delta > 0 {
                        s.bet = s.bet.saturating_add(delta);
                    }
                    if let Some(p) = chip(&m["pot"]) {
                        self.pot = p;
                    }
                    if let Some(ns) = next_street {
                        let saw_flop_if_sd = act_str != "fold"
                            && self.seats.iter().filter(|s| s.active && !s.folded).count() >= 2;
                        self.advance_street(ns, saw_flop_if_sd);
                    }
                }
            }
            "hand_result" => {
                for w in m["winners"].as_array().into_iter().flatten() {
                    if let Some(i) = seat(&w["seat"])
                        && let Some(nm) = w["name"].as_str().filter(|n| !n.is_empty())
                    {
                        self.names[i] = nm.to_owned();
                    }
                }
                let sd = m["shown_cards"]
                    .as_object()
                    .is_some_and(|o| o.len() >= 2)
                    || (self.street >= 3
                        || self.board.as_ref().is_some_and(|b| b.len() == 5))
                        && self.seats.iter().filter(|s| s.active && !s.folded).count() >= 2;
                self.hud.finish_hand(sd);
                if let Some(fs) = m["final_stacks"].as_object() {
                    for i in 0..6 {
                        let stack = fs.get(&i.to_string()).and_then(chip).unwrap_or(0);
                        if stack == 0 {
                            self.seats[i].active = false;
                            self.seats[i].stack = 0;
                            self.names[i].clear();
                        } else {
                            self.seats[i].stack = stack;
                        }
                    }
                }
                self.hole = None;
                self.board = None;
            }
            _ => (),
        }
    }
    pub fn restore(&mut self, m: &Value) {
        let mut events = m["replayed_events"]
            .as_array()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        events.sort_by_key(|e| e["table_seq"].as_u64());
        for e in events {
            if e["table_seq"].as_u64().is_some_and(|s| s > self.seq) {
                self.observe(e);
                self.seq = e["table_seq"].as_u64().unwrap();
            }
        }
        let s = &m["snapshot"];
        if let Some(id) = m["hand_id"].as_str() {
            self.hand(id);
        }
        self.observe(s);
        self.seq = m["to_table_seq"]
            .as_u64()
            .or_else(|| s["table_seq"].as_u64())
            .unwrap_or(self.seq);
    }
    pub fn view(&self, l: Legal) -> Option<View> {
        let hero = self.hero?;
        let board = self.board.clone()?;
        let hole = self.hole?;
        if !matches!(board.len(), 0 | 3 | 4 | 5)
            || (mask(&board) | mask(&hole)).count_ones() as usize != board.len() + 2
        {
            return None;
        }
        let opponents: Vec<_> = (0..6)
            .filter(|&i| i != hero && self.seats[i].active && !self.seats[i].folded)
            .collect();
        if opponents.is_empty() {
            return None;
        }
        let s = self.seats[hero];
        let deepest = opponents
            .iter()
            .map(|&j| self.seats[j].stack.saturating_add(self.seats[j].bet))
            .max()?;
        let late = self.button.is_some_and(|b| {
            hero == b
                || (opponents.len() > 2
                    && (1..6)
                        .map(|x| (hero + x) % 6)
                        .find(|&i| self.seats[i].active)
                        == Some(b))
        });
        let bet = l
            .raise
            .map(|(_, hi)| hi.saturating_sub(s.stack))
            .unwrap_or(s.bet);
        let mut pot = self.pot;
        if let Some(cap) = s.start_stack {
            let entries: Option<Vec<_>> = self
                .seats
                .iter()
                .filter(|p| p.active)
                .map(|p| p.start_stack?.checked_sub(p.stack))
                .collect();
            if let Some(entries) = entries
                && entries.iter().map(|&x| u64::from(x)).sum::<u64>() == u64::from(pot)
            {
                pot = entries.iter().map(|&x| x.min(cap)).sum();
            }
        }
        Some(View {
            hole,
            board,
            bb: if self.bb == 0 { 20 } else { self.bb },
            pot,
            bet,
            stack: s.stack,
            effective: s.stack.min(deepest.saturating_sub(bet)),
            late,
            ranges: opponents.iter().map(|&i| self.seats[i].range).collect(),
            legal: l,
        })
    }
    pub fn pressure_from(&self, history: &State) -> crate::defense::Pressure {
        history.defense.estimate(
            self.seats
                .iter()
                .enumerate()
                .filter(|(i, s)| Some(*i) != self.hero && s.active && !s.folded && s.stack > 0)
                .map(|(i, s)| (self.names[i].as_str(), s.range > 0.)),
        )
    }

    pub fn priced_pressure_from(&self, history: &State) -> crate::defense::Pressure {
        history.defense.with_calling_evidence(
            self.seats
                .iter()
                .enumerate()
                .filter(|(i, s)| Some(*i) != self.hero && s.active && !s.folded && s.stack > 0)
                .map(|(i, s)| (self.names[i].as_str(), s.range > 0.)),
        )
    }

    pub fn postflop_pressure_from(&self, history: &State) -> crate::defense::Pressure {
        history.defense.postflop_pressure(
            self.seats
                .iter()
                .enumerate()
                .filter(|(i, s)| Some(*i) != self.hero && s.active && !s.folded)
                .map(|(i, _)| self.names[i].as_str()),
        )
    }

    pub fn v2_context(&self, l: Legal, history: &State) -> Option<V2Context> {
        let v = self.view(l)?;
        let hero = self.hero?;
        // Build active seat order clockwise from button
        let active_seats: Vec<usize> = (0..6)
            .filter(|&i| self.seats[i].active)
            .collect();
        let n_active = active_seats.len().max(2);
        let btn = self.button.unwrap_or(active_seats[0]);
        let mut order = Vec::with_capacity(n_active);
        for step in 0..6 {
            let s = (btn + step) % 6;
            if self.seats[s].active {
                order.push(s);
            }
        }
        let pos_of = |seat_idx: usize| -> Pos {
            let off = order.iter().position(|&s| s == seat_idx).unwrap_or(0);
            Pos::from_offset_from_btn(off, order.len().max(2))
        };
        let pos = pos_of(hero);

        // Postflop acting order: SB first, BTN last
        let post_ord = |seat_idx: usize| -> usize {
            let off = order.iter().position(|&s| s == seat_idx).unwrap_or(0);
            if off == 0 { order.len() } else { off }
        };
        let hero_ord = post_ord(hero);
        let actors_behind = (0..6)
            .filter(|&j| {
                j != hero
                    && self.seats[j].active
                    && !self.seats[j].folded
                    && self.seats[j].stack > 0
                    && post_ord(j) > hero_ord
            })
            .count() as u8;
        let in_position = actors_behind == 0;

        let dead = mask(&v.hole) | mask(&v.board);
        let b3_info = (v.board.len() >= 3).then(|| BoardFast::analyze(&v.board[..3]));
        let b4_info = (v.board.len() >= 4).then(|| BoardFast::analyze(&v.board[..4]));
        let b5_info = (v.board.len() == 5).then(|| BoardFast::analyze(&v.board[..5]));

        let mut opponents = Vec::with_capacity(5);
        for j in (0..6).filter(|&i| i != hero && self.seats[i].active && !self.seats[i].folded) {
            let j_hud = history.hud.get(&self.names[j]);
            let j_pos = pos_of(j);
            let mut pf_line = self.pf_line[j];
            // Fallback inference if we reconnected mid-hand without full preflop replay
            if pf_line == PreflopLine::Unacted && self.seats[j].range > 0.0 {
                pf_line = if self.seats[j].range >= 0.8 {
                    PreflopLine::OpenRaise
                } else {
                    PreflopLine::ColdCall
                };
            }
            let mut range = ComboRange::from_preflop(j_pos, pf_line, &j_hud, dead);
            if !v.board.is_empty() {
                for &(st, kind, ratio) in &self.post_actions[j] {
                    let (b_slice, bi) = match st {
                        1 if v.board.len() >= 3 => (&v.board[..3], b3_info.as_ref().unwrap()),
                        2 if v.board.len() >= 4 => (&v.board[..4], b4_info.as_ref().unwrap()),
                        3 if v.board.len() == 5 => (&v.board[..5], b5_info.as_ref().unwrap()),
                        _ => continue,
                    };
                    range.update_postflop(b_slice, mask(b_slice), bi, kind, ratio, st, &j_hud);
                }
            }
            opponents.push(V2Opp {
                seat: j,
                pos: j_pos,
                stack: self.seats[j].stack,
                bet: self.seats[j].bet,
                preflop_line: pf_line,
                hud: j_hud,
                range,
                is_aggressor: self.street_aggressor == Some(j),
                is_live: self.seats[j].stack > 0,
            });
        }

        // Infer preflop_raises if mid-hand without preflop action log
        let pf_raises = if self.pf_raises == 0
            && v.board.is_empty()
            && v.bet.saturating_add(l.call.unwrap_or(0)) > v.bb
        {
            1
        } else {
            self.pf_raises
        };
        let street_raises = if self.street_raises == 0 && l.call.unwrap_or(0) > 0 {
            1
        } else {
            self.street_raises
        };

        Some(V2Context {
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
            preflop_raises: pf_raises,
            limpers: self.limpers,
            hero_was_pfa: self.pfa_seat == Some(hero),
            street_raises,
            facing_check_raise: street_raises >= 2
                && self
                    .street_aggressor
                    .is_some_and(|a| a != hero && self.checked_street[a]),
            opponents,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub hand: String,
    pub token: String,
}
impl Turn {
    pub fn read(hand: &Value, token: &Value) -> Option<Self> {
        Some(Self {
            hand: hand.as_str().filter(|s| !s.is_empty())?.to_owned(),
            token: token.as_str().filter(|s| !s.is_empty())?.to_owned(),
        })
    }
    pub fn payload(&self, action: Action, id: &str) -> Value {
        let name = match action {
            Action::Raise(_) => "raise",
            Action::Call => "call",
            Action::Check => "check",
            Action::Fold => "fold",
            Action::AllIn => "all_in",
        };
        let mut m = json!({"type":"action","hand_id":self.hand,"turn_token":self.token,"client_action_id":id,"action":name});
        if let Action::Raise(n) = action {
            m["amount"] = json!(n);
        }
        m
    }
}

pub fn features(v: &View) -> Value {
    json!({"board":v.board.iter().map(|c|c.text()).collect::<Vec<_>>(),"hole":v.hole.map(|c|c.text()),"pot":v.pot,"bb":v.bb,"bet":v.bet,"stack":v.stack,"effective":v.effective,"late":v.late,"ranges":v.ranges,"legal":v.legal})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resync_and_rotating_positions_have_the_same_public_history() {
        let mut live = State::default();
        let mut offline = State::default();
        for hand in 1..=18 {
            let id = hand.to_string();
            let base = hand * 4;
            let events = [
                json!({"table_seq":base,"type":"hand_start","hand_id":id,"seat":2}),
                json!({"table_seq":base+1,"type":"player_action","hand_id":id,"street":"preflop","seat":2,"name":"hero","action":"raise","to_call_before":20}),
                json!({"table_seq":base+2,"type":"player_action","hand_id":id,"street":"flop","seat":4,"name":"defender","action":"call","to_call_before":60}),
            ];
            for e in &events {
                offline.observe(e);
            }
            let replay = json!({"hand_id":id,"to_table_seq":base+2,"snapshot":{"type":"table_state","hand_id":id,"board":["Ah","2d","3c"]},"replayed_events":[events[2],events[1],events[0]]});
            live.restore(&replay);
            live.restore(&replay);
        }
        let snap = json!({"type":"table_state","hand_id":"next","hero":{"seat":0},"board":[],"seats":[{"seat":1,"name":"defender","stack":100,"in_hand":true,"folded":false}]});
        offline.observe(&snap);
        live.observe(&snap);
        let a = live.pressure_from(&live);
        let b = live.pressure_from(&offline);
        assert!(a.supported && a.fold_through < 0.3);
        assert_eq!(a.fold_through, b.fold_through);
        let mut new = snap;
        new["seats"][0]["name"] = json!("new player");
        live.observe(&new);
        assert!(!live.pressure_from(&live).supported);
        live.table("other");
        assert!(!live.pressure_from(&live).supported);
    }
    #[test]
    fn removes_unwinnable_chips_only_with_reconciled_ledger() {
        let mut s = State::default();
        let mut snap = json!({"type":"table_state","hand_id":"x","board":[],"pot":30,"dealer_seat":0,"big_blind":20,"hero":{"seat":0,"hole_cards":["Ac","Qc"]},"seats":[{"seat":0,"in_hand":true,"stack":40,"bet":0},{"seat":1,"in_hand":true,"stack":190,"bet":10},{"seat":2,"in_hand":true,"stack":180,"bet":20}]});
        s.observe(&snap);
        snap["pot"] = json!(200);
        snap["board"] = json!(["2d", "4s", "7h"]);
        for i in 1..3 {
            snap["seats"][i]["stack"] = json!(100);
            snap["seats"][i]["bet"] = json!(0);
        }
        s.observe(&snap);
        assert_eq!(s.view(Legal::default()).unwrap().pot, 80);
        snap["pot"] = json!(201);
        s.observe(&snap);
        assert_eq!(s.view(Legal::default()).unwrap().pot, 201);
    }

    #[test]
    fn live_street_transitions_preserve_pf_lines_aggressor_and_hud_and_clear_busted_seats() {
        let mut s = State::default();
        s.table("t1");
        s.observe(&json!({
            "type": "table_joined",
            "table_id": "t1",
            "seat": 1,
            "players": [
                {"seat": 0, "name": "OppA", "stack": 1000, "status": "active"},
                {"seat": 1, "name": "ramsay", "stack": 1000, "status": "active"},
                {"seat": 2, "name": "OppB", "stack": 1000, "status": "active"},
                {"seat": 3, "name": "OppC", "stack": 1000, "status": "active"}
            ]
        }));
        // Hand 1: OppA (seat 0) opens, Hero (seat 1) 3-bets, OppB/OppC fold, OppA calls (closing preflop -> street="flop").
        // Then OppA checks flop BEFORE hero gets your_turn (board still [] when player_action arrives!).
        s.observe(&json!({"type": "hand_start", "hand_id": "h1", "seat": 1, "dealer_seat": 1, "blinds": {"small_blind": 10, "big_blind": 20}}));
        s.observe(&json!({"type": "hole_cards", "hand_id": "h1", "cards": ["As", "Ks"]}));
        s.observe(&json!({"type": "player_action", "hand_id": "h1", "seat": 0, "name": "OppA", "action": "raise", "amount": 50, "street": "preflop", "to_call_before": 20, "contribution_delta": 50, "pot_before": 30, "pot": 80, "stack": 950}));
        s.observe(&json!({"type": "player_action", "hand_id": "h1", "seat": 1, "name": "ramsay", "action": "raise", "amount": 150, "street": "preflop", "to_call_before": 50, "contribution_delta": 150, "pot_before": 80, "pot": 230, "stack": 850}));
        s.observe(&json!({"type": "player_action", "hand_id": "h1", "seat": 2, "name": "OppB", "action": "fold", "street": "preflop", "to_call_before": 140, "contribution_delta": 0, "pot_before": 230, "pot": 230, "stack": 990}));
        s.observe(&json!({"type": "player_action", "hand_id": "h1", "seat": 3, "name": "OppC", "action": "fold", "street": "preflop", "to_call_before": 130, "contribution_delta": 0, "pot_before": 230, "pot": 230, "stack": 980}));
        s.observe(&json!({"type": "player_action", "hand_id": "h1", "seat": 0, "name": "OppA", "action": "call", "street": "flop", "to_call_before": 100, "contribution_delta": 100, "pot_before": 230, "pot": 330, "stack": 850}));
        // Flop action by OppA arrives BEFORE your_turn updates board!
        s.observe(&json!({"type": "player_action", "hand_id": "h1", "seat": 0, "name": "OppA", "action": "check", "street": "flop", "to_call_before": null, "contribution_delta": 0, "pot_before": 330, "pot": 330, "stack": 850}));
        s.observe(&json!({
            "type": "your_turn",
            "hand_id": "h1",
            "pot": 330,
            "community_cards": ["Ad", "Jd", "7h"],
            "players": [
                {"seat": 0, "stack": 850, "bet": 0, "status": "active"},
                {"seat": 1, "stack": 850, "bet": 0, "status": "active"}
            ]
        }));
        let l_flop = Legal { fold: true, check: true, call: None, raise: Some((20, 850)), jam: Some(850) };
        let ctx = s.v2_context(l_flop, &s).unwrap();
        assert_eq!(ctx.opponents.len(), 1);
        assert_eq!(ctx.opponents[0].preflop_line, PreflopLine::Call3Bet, "flop check must NOT overwrite Call3Bet with BbCheck");
        assert!(ctx.hero_was_pfa);
        assert_eq!(ctx.street_raises, 0);

        // Hero bets flop, OppA check-raises flop -> facing_check_raise MUST be true!
        s.observe(&json!({"type": "player_action", "hand_id": "h1", "seat": 1, "name": "ramsay", "action": "raise", "amount": 200, "street": "flop", "to_call_before": null, "contribution_delta": 200, "pot_before": 330, "pot": 530, "stack": 650}));
        s.observe(&json!({"type": "player_action", "hand_id": "h1", "seat": 0, "name": "OppA", "action": "raise", "amount": 500, "street": "flop", "to_call_before": 200, "contribution_delta": 500, "pot_before": 530, "pot": 1030, "stack": 350}));
        let l_xr = Legal { fold: true, check: false, call: Some(300), raise: Some((800, 850)), jam: Some(850) };
        let ctx_xr = s.v2_context(l_xr, &s).unwrap();
        assert!(ctx_xr.facing_check_raise, "OppA checked then raised -> facing_check_raise must be true");
        assert_eq!(ctx_xr.street_raises, 2);
        assert!(ctx_xr.opponents[0].is_aggressor);

        // Hand 1 ends with OppB and OppC busted (0 chips).
        s.observe(&json!({
            "type": "hand_result",
            "hand_id": "h1",
            "final_stacks": {"0": 1700, "1": 300, "2": 0, "3": 0},
            "shown_cards": {"0": ["6d", "6c"], "1": ["As", "Ks"]}
        }));

        // Hand 2: HU between seat 0 and seat 1 (seats 2 & 3 busted!).
        // Also test OppA betting turn BEFORE hero gets your_turn (must NOT wipe street_aggressor!).
        s.observe(&json!({"type": "hand_start", "hand_id": "h2", "seat": 1, "dealer_seat": 1, "blinds": {"small_blind": 10, "big_blind": 20}}));
        s.observe(&json!({"type": "hole_cards", "hand_id": "h2", "cards": ["Ah", "9h"]}));
        s.observe(&json!({"type": "player_action", "hand_id": "h2", "seat": 1, "name": "ramsay", "action": "raise", "amount": 50, "street": "preflop", "to_call_before": 10, "contribution_delta": 40, "pot_before": 30, "pot": 70, "stack": 250}));
        s.observe(&json!({"type": "player_action", "hand_id": "h2", "seat": 0, "name": "OppA", "action": "call", "street": "flop", "to_call_before": 30, "contribution_delta": 30, "pot_before": 70, "pot": 100, "stack": 1650}));
        s.observe(&json!({"type": "player_action", "hand_id": "h2", "seat": 0, "name": "OppA", "action": "raise", "amount": 50, "street": "flop", "to_call_before": null, "contribution_delta": 50, "pot_before": 100, "pot": 150, "stack": 1600}));
        s.observe(&json!({
            "type": "your_turn",
            "hand_id": "h2",
            "pot": 150,
            "community_cards": ["Kd", "3d", "Qh"],
            "players": [
                {"seat": 0, "stack": 1600, "bet": 50, "status": "active"},
                {"seat": 1, "stack": 250, "bet": 0, "status": "active"}
            ]
        }));
        let l_h2 = Legal { fold: true, check: false, call: Some(50), raise: Some((100, 250)), jam: Some(250) };
        let ctx_h2 = s.v2_context(l_h2, &s).unwrap();
        assert_eq!(ctx_h2.opponents.len(), 1, "busted seats 2 & 3 must NOT appear as ghost opponents");
        assert_eq!(ctx_h2.opponents[0].preflop_line, PreflopLine::ColdCall, "OppA flop donk must NOT overwrite ColdCall with ThreeBet");
        assert!(ctx_h2.opponents[0].is_aggressor, "your_turn board update must NOT wipe street_aggressor");
        assert_eq!(ctx_h2.street_raises, 1);
        assert!(ctx_h2.hero_was_pfa, "OppA flop donk must NOT overwrite hero_was_pfa");
        assert!(ctx_h2.opponents[0].hud.hands >= 1.0, "OppA HUD must persist across hands without name wipe");
    }

    #[test]
    fn openpoker_all_in_with_zero_delta_counts_as_raise() {
        let mut s = State::default();
        s.table("t2");
        s.observe(&json!({"type": "hand_start", "hand_id": "h3", "seat": 1, "dealer_seat": 2, "blinds": {"small_blind": 10, "big_blind": 20}}));
        s.observe(&json!({"type": "hole_cards", "hand_id": "h3", "cards": ["As", "Ad"]}));
        s.observe(&json!({
            "type": "player_action",
            "hand_id": "h3",
            "seat": 0,
            "name": "Shover",
            "action": "all_in",
            "amount": null,
            "street": "preflop",
            "to_call_before": 20,
            "contribution_delta": 0,
            "pot_before": 30,
            "pot": 30,
            "stack_before": 2000,
            "stack_after": 2000,
            "stack": 2000
        }));
        s.observe(&json!({
            "type": "your_turn",
            "hand_id": "h3",
            "pot": 2030,
            "community_cards": [],
            "players": [
                {"seat": 0, "stack": 0, "bet": 2000, "status": "all_in"},
                {"seat": 1, "stack": 2000, "bet": 0, "status": "active"}
            ]
        }));
        let l = Legal { fold: true, check: false, call: Some(2000), raise: None, jam: None };
        let ctx = s.v2_context(l, &s).unwrap();
        assert_eq!(ctx.preflop_raises, 1);
        assert_eq!(ctx.limpers, 0);
        assert_eq!(ctx.opponents[0].preflop_line, PreflopLine::OpenRaise);
        let d = crate::v2::decide_v2(&ctx, 1, 256);
        assert_eq!(d.action, Action::Call, "AA facing 100 BB open-shove MUST call, got {d:?}");
    }
}
