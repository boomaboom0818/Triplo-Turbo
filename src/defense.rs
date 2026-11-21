//! Recent public replies to hero's first preflop raise. Commit samples only on
//! the next hand. A reraise ends attribution: later players face someone else.
use std::collections::VecDeque;

#[derive(Clone, Copy, Default, Debug)]
pub struct Pressure {
    pub fold_through: f64,
    pub reraise: f64,
    pub supported: bool,
    pub sticky_postflop: bool,
    pub disciplined_field: bool,
}

#[derive(Default)]
pub struct Defense {
    past: VecDeque<Vec<Reply>>,
    samples: Vec<Reply>,
    postflop: VecDeque<Vec<(String, bool)>>,
    paid: Vec<(String, bool)>,
    invested: [bool; 6],
    prior: [bool; 6],
    tracking: bool,
    opened: bool,
    preflop: bool,
}

struct Reply {
    name: String,
    invested: bool,
    defended: bool,
    raised: bool,
}

impl Defense {
    pub fn next_hand(&mut self) {
        if self.past.len() == 80 {
            self.past.pop_front();
        }
        self.past.push_back(std::mem::take(&mut self.samples));
        if self.postflop.len() == 80 {
            self.postflop.pop_front();
        }
        self.postflop.push_back(std::mem::take(&mut self.paid));
        self.invested = [false; 6];
        self.tracking = false;
        self.opened = false;
        self.preflop = true;
    }
    pub fn end_street(&mut self) {
        self.preflop = false;
    }
    pub fn action(&mut self, m: &serde_json::Value, hero: Option<usize>, i: usize) {
        // Open Poker reports the street *after* applying the action, even on
        // the final preflop fold or call. Never infer a raise from a short jam.
        let street = m["street"].as_str();
        if !self.preflop
            && Some(i) != hero
            && matches!(street, Some("flop" | "turn" | "river" | "showdown"))
        {
            let price = m["to_call_before"].as_u64().unwrap_or(0);
            let pot = m["pot_before"].as_u64().unwrap_or(0);
            let kind = m["action"].as_str();
            if price > 0
                && pot > 0
                && price >= pot.div_ceil(8)
                && price <= pot
                && m["reason"].is_null()
                && matches!(kind, Some("fold" | "call" | "raise" | "all_in"))
                && let Some(name) = m["name"].as_str().filter(|n| !n.is_empty())
                && self.paid.len() < 5
                && !self.paid.iter().any(|p| p.0 == name)
            {
                self.paid.push((name.into(), kind == Some("call")));
            }
        }
        if !self.preflop || street.is_none() {
            return;
        }
        let call = m["to_call_before"].as_u64().unwrap_or(0);
        let kind = m["action"].as_str();
        let raise = kind == Some("raise")
            || kind == Some("all_in") && m["contribution_delta"].as_u64().is_some_and(|d| d > call);
        if Some(i) == hero && raise && !self.opened && street == Some("preflop") {
            self.prior = self.invested;
            self.opened = true;
            self.tracking = true;
        } else if Some(i) == hero {
            self.tracking = false;
        } else if self.tracking && Some(i) != hero {
            if let Some(name) = m["name"].as_str().filter(|n| !n.is_empty())
                && m["reason"].is_null()
                && call > 0
                && matches!(kind, Some("fold" | "raise" | "call" | "all_in"))
                && self.samples.len() < 5
                && !self.samples.iter().any(|x| x.name == name)
            {
                self.samples.push(Reply {
                    name: name.into(),
                    invested: self.prior[i],
                    defended: kind != Some("fold"),
                    raised: raise,
                });
            }
            if raise {
                self.tracking = false;
            }
        }
        self.invested[i] |= matches!(kind, Some("call" | "raise" | "all_in"));
        if street != Some("preflop") {
            self.end_street();
        }
    }

    pub fn estimate<'a>(&self, opponents: impl Iterator<Item = (&'a str, bool)>) -> Pressure {
        let mut fold = 1.;
        let mut no_raise = 1.;
        let mut support = 0;
        let mut experienced = false;
        for (name, invested) in opponents {
            let (mut n, mut defended, mut raised) = (0, 0, 0);
            for x in self
                .past
                .iter()
                .flatten()
                .filter(|x| x.name == name && x.invested == invested)
            {
                n += 1;
                defended += usize::from(x.defended);
                raised += usize::from(x.raised);
            }
            // Shrink small samples; unknown seats alone cannot trigger a change.
            let d = (defended as f64 + 1.5) / (n as f64 + 8.);
            let r = (raised as f64 + 0.32) / (n as f64 + 8.);
            fold *= 1. - d;
            no_raise *= 1. - r;
            support += n;
            experienced |= n >= 5;
        }
        Pressure {
            fold_through: fold,
            reraise: 1. - no_raise,
            supported: experienced && support >= 12,
            ..Default::default()
        }
    }

    pub fn postflop_pressure<'a>(&self, names: impl Iterator<Item = &'a str>) -> Pressure {
        let (mut total, mut sticky) = (0usize, 0usize);
        let mut disciplined = 0usize;
        for name in names {
            total += 1;
            let (mut n, mut called) = (0usize, 0usize);
            for x in self.postflop.iter().flatten().filter(|x| x.0 == name) {
                n += 1;
                called += usize::from(x.1);
            }
            sticky += usize::from(n >= 14 && (called + 1) * 100 >= (n + 4) * 83);
            disciplined += usize::from(n >= 12 && (called + 1) * 100 <= (n + 4) * 68);
        }
        Pressure {
            sticky_postflop: total >= 3 && sticky >= total.div_ceil(2),
            disciplined_field: total >= 2 && sticky == 0 && disciplined >= total.div_ceil(2).max(2),
            ..Default::default()
        }
    }

    pub fn with_calling_evidence<'a>(
        &self,
        opponents: impl Iterator<Item = (&'a str, bool)> + Clone,
    ) -> Pressure {
        let mut pre = self.estimate(opponents.clone());
        pre.disciplined_field = self
            .postflop_pressure(opponents.map(|x| x.0))
            .disciplined_field;
        pre
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_prior_meaningful_calls_count_toward_sticky_field() {
        let mut h = Defense::default();
        let pressure = |h: &Defense| {
            h.postflop_pressure(["A", "B", "C"].into_iter())
                .sticky_postflop
        };
        for n in 0..14 {
            h.next_hand();
            // A preflop terminal transition reports the next street, not an observed postflop call.
            h.action(&json!({"street":"flop","to_call_before":50,"pot_before":100,"action":"call","name":"B"}),Some(0),1);
            h.end_street();
            for (i, name, action, cost) in [
                (1, "A", "call", 60),
                (2, "B", "call", 60),
                (3, "C", "call", 4),
                (3, "C", "raise", 80),
            ] {
                h.action(&json!({"street":"flop","to_call_before":cost,"pot_before":100,"action":action,"name":name}),Some(0),i);
            }
            assert!(!pressure(&h), "must not include the current hand {n}");
        }
        h.next_hand();
        assert!(pressure(&h));
        assert!(
            !h.postflop_pressure(["A", "B", "C"].into_iter())
                .disciplined_field
        );
        assert!(
            !h.postflop_pressure(["A", "C", "replacement"].into_iter())
                .sticky_postflop
        );
        for _ in 0..81 {
            h.next_hand();
        }
        assert!(!pressure(&h));
    }

    #[test]
    fn disciplined_field_requires_observed_folds_and_does_not_follow_empty_seats() {
        let mut d = Defense::default();
        for _ in 0..12 {
            d.next_hand();
            d.end_street();
            for (i, name) in [(1, "A"), (2, "B")] {
                d.action(&json!({"type":"player_action","name":name,"street":"flop","action":"fold","to_call_before":50,"pot_before":120}),Some(0),i);
            }
        }
        assert!(
            !d.postflop_pressure(["A", "B", "unknown"].into_iter())
                .disciplined_field
        );
        d.next_hand();
        assert!(
            d.postflop_pressure(["A", "B", "unknown"].into_iter())
                .disciplined_field
        );
        assert!(
            !d.postflop_pressure(["A", "new", "unknown"].into_iter())
                .disciplined_field
        );
    }

    #[test]
    fn attributes_only_direct_priced_replies_and_expires_them() {
        let mut h = Defense::default();
        for _ in 0..18 {
            h.next_hand();
            for (i, name, action, street, owed, paid) in [
                (1, "limper", "call", "preflop", 20, 20),
                (0, "hero", "raise", "preflop", 20, 60),
                (2, "short", "all_in", "preflop", 60, 40),
                (3, "cold", "raise", "preflop", 60, 180),
                (1, "limper", "call", "flop", 160, 160),
            ] {
                h.action(&json!({"name":name,"action":action,"street":street,"to_call_before":owed,"contribution_delta":paid}),Some(0),i);
            }
        }
        let pressure = |h: &Defense, n| h.estimate([(n, false)].into_iter());
        assert!(pressure(&h, "cold").reraise > 0.6);
        assert!(pressure(&h, "short").supported && pressure(&h, "short").reraise < 0.1);
        assert!(!h.estimate([("limper", true)].into_iter()).supported);
        assert!(!pressure(&h, "replacement").supported);
        for _ in 0..81 {
            h.next_hand();
        }
        assert!(!pressure(&h, "cold").supported);
    }
    #[test]
    fn counts_the_preflop_closing_action_but_no_later_street() {
        let mut h = Defense::default();
        for _ in 0..20 {
            h.next_hand();
            h.action(&json!({"street":"preflop","action":"raise"}), Some(0), 0);
            h.action(
                &json!({"street":"flop","action":"call","to_call_before":40,"name":"A"}),
                Some(0),
                1,
            );
            h.action(
                &json!({"street":"flop","action":"raise","to_call_before":80,"name":"B"}),
                Some(0),
                2,
            );
        }
        assert!(h.estimate([("A", false)].into_iter()).supported);
        assert!(!h.estimate([("B", false)].into_iter()).supported);
    }
}
