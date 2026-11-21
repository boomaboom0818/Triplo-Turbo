use crate::cards::{Card, deck, evaluate, mask};
use crate::policy::{Action, Chips, Legal, View};
use rand::{Rng, SeedableRng, seq::SliceRandom};
use rand_chacha::ChaCha8Rng;
use rayon::prelude::*;
use serde_json::{Value, json};
use std::time::Instant;

mod sessions;
mod v2_bench;
pub use sessions::{guard_bench, guard_fit, lab_bench, price_bench, strategy_bench};
pub use v2_bench::{bench_v2, debug_hands, tune_v2};

const BB: Chips = 20;
#[derive(Clone)]
struct Hand {
    n: usize,
    button: usize,
    stack: [Chips; 6],
    total: [Chips; 6],
    bet: [Chips; 6],
    folded: [bool; 6],
    acted: [Option<Chips>; 6],
    tight: [f64; 6],
    street: usize,
    actor: Option<usize>,
    target: Chips,
    increment: Chips,
    holes: [[Card; 2]; 6],
    runout: [Card; 5],
}
impl Hand {
    fn new(n: usize, button: usize, stacks: [Chips; 6], seed: u64) -> Self {
        assert!(
            (2..=6).contains(&n)
                && button < n
                && stacks[..n].iter().all(|&x| x > 0 && x <= 1_000_000)
        );
        let mut d = deck();
        d.shuffle(&mut ChaCha8Rng::seed_from_u64(seed));
        let mut h = Self {
            n,
            button,
            stack: stacks,
            total: [0; 6],
            bet: [0; 6],
            folded: [false; 6],
            acted: [None; 6],
            tight: [0.0; 6],
            street: 0,
            actor: None,
            target: BB,
            increment: BB,
            holes: std::array::from_fn(|i| [d[i], d[i + n]]),
            runout: d[2 * n..2 * n + 5].try_into().unwrap(),
        };
        let sb = (button + usize::from(n > 2)) % n;
        let big = (sb + 1) % n;
        h.pay(sb, BB / 2);
        h.pay(big, BB);
        h.advance(big);
        h
    }
    fn pay(&mut self, i: usize, want: Chips) {
        let cost = want.min(self.stack[i]);
        self.stack[i] -= cost;
        self.total[i] += cost;
        self.bet[i] += cost;
    }
    fn active(&self) -> impl Iterator<Item = usize> + Clone + '_ {
        (0..self.n).filter(|&i| !self.folded[i])
    }
    fn advance(&mut self, mut prev: usize) {
        self.actor = None;
        loop {
            if self.active().count() <= 1 {
                return;
            }
            let live = self.active().filter(|&i| self.stack[i] > 0).count();
            for step in 1..=self.n {
                let i = (prev + step) % self.n;
                if self.folded[i] || self.stack[i] == 0 {
                    continue;
                }
                let owes = if live > 1 {
                    self.bet[i] < self.target || self.acted[i].is_none()
                } else {
                    self.active().any(|j| j != i && self.bet[j] > self.bet[i])
                };
                if owes {
                    self.actor = Some(i);
                    return;
                }
            }
            if self.street == 3 || live < 2 {
                return;
            }
            self.street += 1;
            self.target = 0;
            self.increment = BB;
            self.bet = [0; 6];
            self.acted = [None; 6];
            prev = self.button;
        }
    }
    fn menu(&self, i: usize) -> Legal {
        // A short big blind still sets the nominal bring-in for other live stacks.
        // If nobody else can act, only an opponent's actual chips can be matched.
        let other_live = self.active().any(|j| j != i && self.stack[j] > 0);
        let target = if other_live {
            self.target
        } else {
            self.active()
                .filter(|&j| j != i)
                .map(|j| self.bet[j])
                .max()
                .unwrap_or(0)
        };
        let owing = target.saturating_sub(self.bet[i]);
        let max = self.stack[i] + self.bet[i];
        let can_raise =
            other_live && self.acted[i].is_none_or(|last| self.target - last >= self.increment);
        let min = self.target + self.increment;
        Legal {
            fold: owing > 0,
            check: owing == 0,
            call: (owing > 0).then_some(owing.min(self.stack[i])),
            raise: (can_raise && max >= min).then_some((min, max)),
            jam: (can_raise && max > self.target).then_some(max),
        }
    }
    fn apply(&mut self, a: Action) -> bool {
        let Some(i) = self.actor else { return false };
        let l = self.menu(i);
        if !l.permits(a) {
            return false;
        }
        match a {
            Action::Fold => self.folded[i] = true,
            Action::Check => (),
            Action::Call => self.pay(i, l.call.unwrap()),
            Action::Raise(_) | Action::AllIn => {
                let to = if let Action::Raise(n) = a {
                    n
                } else {
                    l.jam.unwrap()
                };
                self.increment = self.increment.max(to - self.target);
                self.target = to;
                self.pay(i, to - self.bet[i]);
                self.tight[i] = 0.82;
            }
        }
        if a == Action::Call {
            self.tight[i] = self.tight[i].max(0.40);
        }
        self.acted[i] = Some(self.target);
        self.advance(i);
        true
    }
    fn view(&self, i: usize) -> View {
        let opp = self.active().filter(|&j| j != i);
        let deepest = opp
            .clone()
            .map(|j| self.stack[j] + self.bet[j])
            .max()
            .unwrap();
        let cap = self.total[i] + self.stack[i]; // exclude layers hero can never contest
        View {
            hole: self.holes[i],
            board: self.runout[..[0, 3, 4, 5][self.street]].to_vec(),
            bb: BB,
            pot: self.total[..self.n].iter().map(|&x| x.min(cap)).sum(),
            bet: self.bet[i],
            stack: self.stack[i],
            effective: self.stack[i].min(deepest.saturating_sub(self.bet[i])),
            late: i == self.button || (self.n > 3 && (i + 1) % self.n == self.button),
            ranges: opp.map(|j| self.tight[j]).collect(),
            legal: self.menu(i),
        }
    }
    fn settle(self) -> [Chips; 6] {
        assert!(self.actor.is_none());
        let board = mask(&self.runout);
        let ranks = std::array::from_fn(|i| evaluate(board | mask(&self.holes[i])));
        let mut out = self.stack;
        let gains = payouts(self.n, self.button, self.total, self.folded, ranks);
        for i in 0..self.n {
            out[i] += gains[i];
        }
        out
    }
}

fn payouts(
    n: usize,
    button: usize,
    total: [Chips; 6],
    folded: [bool; 6],
    ranks: [u32; 6],
) -> [Chips; 6] {
    let mut levels = total;
    levels[..n].sort_unstable();
    let mut prev = 0;
    let mut out = [0; 6];
    for &level in &levels[..n] {
        if level == prev {
            continue;
        }
        let mut cont = (0..n).filter(|&s| total[s] >= level);
        let k = cont.clone().count();
        let pot = (level - prev) * k as Chips;
        prev = level;
        if k == 1 {
            out[cont.next().unwrap()] += pot;
            continue;
        }
        let best = cont
            .clone()
            .filter(|&i| !folded[i])
            .map(|i| ranks[i])
            .max()
            .expect("contested layer has eligible seat");
        let wins = |i: usize| total[i] >= level && !folded[i] && ranks[i] == best;
        let k = cont.filter(|&i| wins(i)).count() as Chips;
        let mut odd = pot % k;
        for i in (1..=n).map(|i| (button + i) % n).filter(|&i| wins(i)) {
            out[i] += pot / k + u32::from(odd > 0);
            odd = odd.saturating_sub(1);
        }
    }
    out
}

fn scripted(v: &View, style: usize, rng: &mut ChaCha8Rng) -> Action {
    let l = v.legal;
    let pair = v.hole[0].rank() == v.hole[1].rank();
    let high = v.hole.iter().map(|c| c.rank()).max().unwrap();
    let made = if v.board.is_empty() {
        pair || high >= 13
    } else {
        evaluate(mask(&v.board) | mask(&v.hole)) >> 24 >= 1
    };
    let price = l.call.unwrap_or(0);
    if style != 0
        && made
        && (style == 2 || high == 14)
        && rng.gen_bool(if style == 2 { 0.42 } else { 0.24 })
        && let Some((lo, hi)) = l.raise
    {
        return Action::Raise((v.bet + price + (v.pot + price) / 2).clamp(lo, hi));
    }
    if l.check {
        return Action::Check;
    }
    if l.call.is_some() && (style == 0 || (made && price <= v.pot) || (price <= BB && high >= 10)) {
        return Action::Call;
    }
    l.passive().unwrap()
}

struct Block {
    chips: i64,
    won: u32,
    profitable: u32,
    latencies: Vec<u64>,
    worst_hand: i64,
}
fn block(seed: u64, n: usize, samples: usize, profile: u8, field: &str) -> Block {
    let mut b = Block {
        chips: 0,
        won: 0,
        profitable: 0,
        latencies: vec![],
        worst_hand: 0,
    };
    // Same deal for every seat rotation; all opponents keep their seat's RNG stream.
    for hero in 0..n {
        let stacks = std::array::from_fn(|i| if i < n { 2000 } else { 0 });
        let mut h = Hand::new(n, 0, stacks, seed);
        let mut used = false;
        let mut rngs: [_; 6] = std::array::from_fn(|i| {
            ChaCha8Rng::seed_from_u64(seed ^ (i as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15))
        });
        while let Some(i) = h.actor {
            let v = h.view(i);
            let a = if i == hero {
                let t = Instant::now();
                let d = crate::policy::with_profile(
                    &v,
                    rngs[i].r#gen(),
                    samples,
                    crate::policy::budgeted_profile(profile, used),
                );
                used |= d.spends_response();
                let a = d.action;
                b.latencies.push(t.elapsed().as_micros() as u64);
                a
            } else if field == "baseline" || (field == "mixed" && (i + n - hero) % 2 == 1) {
                crate::policy::with_profile(&v, rngs[i].r#gen(), samples, 0).action
            } else {
                scripted(&v, (i + n - hero - 1) % n % 3, &mut rngs[i])
            };
            assert!(h.apply(a));
        }
        let remaining = h.stack[hero];
        let other_max = (0..n)
            .filter(|&i| i != hero)
            .map(|i| h.total[i])
            .max()
            .unwrap();
        let refund = h.total[hero].saturating_sub(other_max);
        let out = h.settle();
        assert_eq!(out[..n].iter().sum::<u32>(), n as u32 * 2000);
        let net = out[hero] as i64 - 2000;
        b.won += u32::from(out[hero] > remaining + refund);
        b.profitable += u32::from(net > 0);
        b.chips += net;
        b.worst_hand = b.worst_hand.min(net);
    }
    b
}
pub fn bench(
    blocks: usize,
    n: usize,
    seed: u64,
    samples: usize,
    profile: u8,
    field: &str,
) -> Value {
    let start = Instant::now();
    let games: Vec<_> = (0..blocks)
        .into_par_iter()
        .map(|i| block(seed.wrapping_add(i as u64), n, samples, profile, field))
        .collect();
    let x: Vec<_> = games
        .iter()
        .map(|b| 100.0 * b.chips as f64 / (n as f64 * BB as f64))
        .collect();
    let mean = x.iter().sum::<f64>() / blocks as f64;
    let se =
        (x.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (blocks - 1) as f64 / blocks as f64)
            .sqrt();
    let mut lat: Vec<_> = games
        .iter()
        .flat_map(|x| x.latencies.iter().copied())
        .collect();
    lat.sort_unstable();
    let won = games.iter().map(|b| b.won as usize).sum::<usize>();
    let win = 100.0 * won as f64 / (blocks * n) as f64;
    let win_se = (games
        .iter()
        .map(|b| (100.0 * b.won as f64 / n as f64 - win).powi(2))
        .sum::<f64>()
        / ((blocks - 1) * blocks) as f64)
        .sqrt();
    json!({"mode":"fixed_100bb","field":field,"policy_profile":profile,"hands":blocks*n,"wins":won,"won_pct":win,"won_pct_block_95pct_ci":[win-1.96*win_se,win+1.96*win_se],"net_positive_hands":games.iter().map(|b|b.profitable as usize).sum::<usize>(),"deal_blocks":blocks,"seats":n,"seed":seed,"samples":samples,"hero_bb_per_100":mean,"block_95pct_ci":[mean-1.96*se,mean+1.96*se],"worst_hand_bb":games.iter().map(|b|b.worst_hand).min().unwrap() as f64/BB as f64,"decisions":lat.len(),"decision_p99_us":lat[(lat.len()-1)*99/100],"elapsed_s":start.elapsed().as_secs_f64()})
}

/// Generate paired action-value samples across randomized lineups and depths.
/// Fields include a calling station, a pressure bot and two frozen equity profiles.
pub fn train(seeds: usize, start: u64) -> Value {
    use std::collections::BTreeMap;
    fn action(
        h: &Hand,
        i: usize,
        hero: usize,
        styles: &[u8; 6],
        rngs: &mut [ChaCha8Rng; 6],
        samples: usize,
    ) -> Action {
        let v = h.view(i);
        let style = styles[i];
        if i == hero || style >= 3 {
            crate::policy::with_profile(
                &v,
                rngs[i].r#gen(),
                samples,
                if i == hero || style == 4 { 14 } else { 0 },
            )
            .action
        } else {
            scripted(&v, style as usize, &mut rngs[i])
        }
    }
    fn finish(
        mut h: Hand,
        first: Action,
        hero: usize,
        styles: &[u8; 6],
        mut rngs: [ChaCha8Rng; 6],
    ) -> u32 {
        assert!(h.apply(first));
        while let Some(i) = h.actor {
            let a = action(&h, i, hero, styles, &mut rngs, 96);
            assert!(h.apply(a));
        }
        h.settle()[hero]
    }
    let pairs: Vec<_> = (0..seeds)
        .into_par_iter()
        .filter_map(|idx| {
            let seed = start.wrapping_add(idx as u64);
            let mut r = ChaCha8Rng::seed_from_u64(seed ^ 0xb7ac102f);
            let hero = r.gen_range(0..6);
            let depth = [50, 100, 200][r.gen_range(0..3)];
            let mut h = Hand::new(6, 0, [depth * BB; 6], seed);
            let styles: [u8; 6] = std::array::from_fn(|_| r.gen_range(0..5));
            let mut rngs = std::array::from_fn(|_| ChaCha8Rng::seed_from_u64(r.r#gen()));
            while let Some(i) = h.actor {
                let a = action(&h, i, hero, &styles, &mut rngs, 96);
                if i == hero {
                    let (key, alt) = crate::policy::probe(&h.view(i), a)?;
                    let baseline = finish(h.clone(), a, hero, &styles, rngs.clone());
                    let alternative = finish(h, alt, hero, &styles, rngs);
                    return Some((key, (alternative as f64 - baseline as f64) / BB as f64));
                }
                assert!(h.apply(a));
            }
            None
        })
        .collect();
    let mut rows: BTreeMap<u32, (usize, f64, f64)> = BTreeMap::new();
    for &(k, d) in &pairs {
        let e = rows.entry(k).or_default();
        e.0 += 1;
        e.1 += d;
        e.2 += d * d;
    }
    let mut enabled = vec![];
    let mut buckets = vec![];
    for (key, (n, s, ss)) in rows {
        let mean = s / n as f64;
        let se = ((ss - s * s / n as f64).max(0.0) / (n.max(2) - 1) as f64 / n as f64).sqrt();
        let lower = mean - 2.58 * se;
        if n >= 100 && lower > 0.05 {
            enabled.push(key);
        }
        buckets.push(json!({"bucket":key,"pairs":n,"mean_gain_bb":mean,"se":se}));
    }
    json!({"schema":1,"base_profile":14,"enabled":enabled,"sampled_hands":seeds,"seed":start,"paired_decisions":pairs.len(),"buckets":buckets})
}

/// Independently screen model actions under two other opponent distributions.
pub fn robust_fit(n: usize, start: u64) -> Value {
    type Rngs = [ChaCha8Rng; 6];
    fn act(h: &Hand, i: usize, hero: usize, styles: &[u8; 6], r: &mut Rngs) -> Action {
        let v = h.view(i);
        if hero == i {
            crate::policy::with_profile(&v, r[i].r#gen(), 96, 17).action
        } else if styles[i] >= 3 {
            crate::policy::with_profile(&v, r[i].r#gen(), 96, if styles[i] == 3 { 0 } else { 14 })
                .action
        } else {
            scripted(&v, styles[i] as usize, &mut r[i])
        }
    }
    fn finish(mut h: Hand, a: Action, hero: usize, styles: &[u8; 6], mut rng: Rngs) -> f64 {
        assert!(h.apply(a));
        while let Some(i) = h.actor {
            let a = act(&h, i, hero, styles, &mut rng);
            assert!(h.apply(a));
        }
        h.settle()[hero] as f64 / BB as f64
    }
    let values: Vec<_> = (0..n)
        .into_par_iter()
        .filter_map(|i| {
            let seed = start + i as u64;
            let mut r = ChaCha8Rng::seed_from_u64(seed ^ 0xfdef5502);
            let hero = r.gen_range(0..6);
            let group = i % 2;
            let depth = [50, 100, 200][r.gen_range(0..3)];
            let mut h = Hand::new(6, 0, [depth * BB; 6], seed);
            let styles = std::array::from_fn(|_| {
                if group == 0 {
                    r.gen_range(0..4)
                } else {
                    r.gen_range(3..5)
                }
            });
            let mut rng = std::array::from_fn(|_| ChaCha8Rng::seed_from_u64(r.r#gen()));
            while let Some(i) = h.actor {
                let a = if i != hero {
                    act(&h, i, hero, &styles, &mut rng)
                } else {
                    let v = h.view(i);
                    let s = rng[i].r#gen();
                    let d = crate::policy::with_profile(&v, s, 96, 17);
                    if r.gen_bool(0.75) {
                        let alt = crate::policy::with_profile(&v, s, 96, 18).action;
                        if alt != d.action && crate::policy::robust_allowed(&v, &d, alt) {
                            let key = crate::policy::response_probe(&v, &d)?.0;
                            let base = finish(h.clone(), d.action, hero, &styles, rng.clone());
                            let alt = finish(h, alt, hero, &styles, rng);
                            return Some((key, group, alt - base));
                        }
                    }
                    d.action
                };
                assert!(h.apply(a));
            }
            None
        })
        .collect();
    let mut cells = std::collections::BTreeMap::new();
    for &(k, g, d) in &values {
        let row = cells.entry(k).or_insert([(0usize, 0f64, 0f64); 2]);
        row[g].0 += 1;
        row[g].1 += d;
        row[g].2 += d * d;
    }
    let mut enabled = vec![];
    for (k, rows) in cells {
        if rows.iter().all(|&(n, s, ss)| {
            if n < 100 {
                return false;
            }
            let mean = s / n as f64;
            let se = ((ss - s * s / n as f64).max(0.0) / (n - 1) as f64 / n as f64).sqrt();
            mean - 1.96 * se >= -0.20
        }) {
            enabled.push(k);
        }
    }
    json!({"schema":1,"base_profile":17,"proposal_profile":18,"sampled_hands":n,"paired_comparisons":values.len(),"seed":start,"enabled":enabled})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn side_pots_ties_and_returns() {
        assert_eq!(
            payouts(
                3,
                0,
                [40, 100, 160, 0, 0, 0],
                [false; 6],
                [3, 2, 1, 0, 0, 0]
            ),
            [120, 120, 60, 0, 0, 0]
        );
        assert_eq!(
            payouts(
                3,
                0,
                [5, 5, 5, 0, 0, 0],
                [false, false, true, false, false, false],
                [4, 4, 9, 0, 0, 0]
            ),
            [7, 8, 0, 0, 0, 0]
        );
    }
    #[test]
    fn short_raises_reopen_only_after_full_increment() {
        let mut h = Hand::new(5, 0, [75, 200, 200, 200, 60, 0], 1);
        assert_eq!(h.actor, Some(3));
        assert!(h.apply(Action::Raise(45)));
        assert!(h.apply(Action::AllIn));
        assert!(h.apply(Action::AllIn));
        assert!(h.apply(Action::Fold));
        assert!(h.apply(Action::Fold));
        assert!(h.menu(3).call == Some(30));
        assert_eq!(h.menu(3).jam, None); /* no other player has chips */
        let mut h = Hand::new(5, 0, [75, 200, 200, 200, 60, 0], 1);
        for a in [
            Action::Raise(45),
            Action::AllIn,
            Action::AllIn,
            Action::Call,
            Action::Fold,
        ] {
            assert!(h.apply(a));
        }
        assert_eq!(h.menu(3).raise, Some((100, 200)));
    }
    #[test]
    fn hu_blinds_and_option() {
        let mut h = Hand::new(2, 0, [100, 100, 0, 0, 0, 0], 2);
        assert_eq!(h.actor, Some(0));
        assert!(!h.apply(Action::Check));
        assert!(h.apply(Action::Call));
        assert_eq!(h.actor, Some(1));
        assert!(h.apply(Action::Check));
        assert_eq!((h.street, h.actor), (1, Some(1)));
    }
    #[test]
    fn short_blind_cost_and_unreopened_player() {
        let h = Hand::new(2, 0, [100, 15, 0, 0, 0, 0], 2);
        assert_eq!(h.menu(0).call, Some(5));
        let mut h = Hand::new(5, 0, [100, 100, 100, 100, 60, 0], 2);
        for a in [
            Action::Raise(45),
            Action::AllIn,
            Action::Call,
            Action::Fold,
            Action::Fold,
        ] {
            assert!(h.apply(a));
        }
        assert_eq!(h.actor, Some(3));
        assert_eq!(h.menu(3).raise, None);
        assert_eq!(h.menu(3).call, Some(15));
    }
    #[test]
    fn random_legal_hands_conserve_chips() {
        let mut r = ChaCha8Rng::seed_from_u64(91);
        for seed in 0..3000 {
            let n = r.gen_range(2..=6);
            let stacks = std::array::from_fn(|i| if i < n { r.gen_range(1..500) } else { 0 });
            let mut h = Hand::new(n, seed % n, stacks, seed as u64);
            let mut turns = 0;
            while let Some(i) = h.actor {
                let l = h.menu(i);
                let mut bag = vec![l.passive().unwrap()];
                if l.call.is_some() {
                    bag.extend([Action::Call; 3]);
                }
                if l.jam.is_some() {
                    bag.push(Action::AllIn);
                }
                if let Some((lo, hi)) = l.raise {
                    bag.push(Action::Raise(r.gen_range(lo..=hi)));
                }
                assert!(h.apply(*bag.choose(&mut r).unwrap()));
                turns += 1;
                assert!(turns < 200);
            }
            assert_eq!(h.settle().iter().sum::<u32>(), stacks.iter().sum::<u32>());
        }
    }
}
