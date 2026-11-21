//! Price-aware cold strategy; uses only the current player-visible view.
use crate::policy::{Action, Decision, View, quality};

pub fn speculative_call_guard(v: &View, d: &Decision, implied: bool) -> Option<Action> {
    if !v.board.is_empty()
        || d.action != Action::Call
        || !matches!(d.reason, "rollout_preflop" | "trained_response")
        || !v.legal.fold
    {
        return None;
    }
    let bb = v.bb.max(1);
    let call = v.legal.call?.min(v.stack);
    if call <= bb {
        return None;
    }
    let [a, b] = v.hole.map(crate::cards::Card::rank);
    if quality(v.hole) >= 0.79 || a == b && a >= 8 {
        return None;
    }
    let price = f64::from(call) / (f64::from(v.pot) + f64::from(call));
    (price >= 0.34
        || implied
            && price >= 0.25
            && call >= bb.saturating_mul(2)
            && v.effective < call.saturating_mul(12))
    .then_some(Action::Fold)
}

pub fn priced(v: &View, wide: bool) -> Decision {
    let l = v.legal;
    let call = l.call.unwrap_or(0).min(v.stack);
    let bb = v.bb.max(1);
    let target = v.bet.saturating_add(call);
    let odds = f64::from(call) / (f64::from(call) + f64::from(v.pot)).max(1.0);
    let (hi, lo) = (
        v.hole[0].rank().max(v.hole[1].rank()),
        v.hole[0].rank().min(v.hole[1].rank()),
    );
    let pair = hi == lo;
    let suited = v.hole[0].suit() == v.hole[1].suit();
    let premium = pair && hi >= 12 || hi == 14 && lo == 13;
    let strong =
        premium || pair && hi >= 9 || hi == 14 && lo >= 12 || suited && hi == 13 && lo == 12;
    let depth = f64::from(v.effective) / f64::from(bb);
    let n = v.ranges.len();
    let passive = l.passive().unwrap();
    let cont = if l.check {
        Action::Check
    } else {
        l.call.map_or(passive, |_| Action::Call)
    };
    let mut action = passive;
    let mut reason = "disciplined_fold";
    let q = quality(v.hole);
    if target <= bb {
        let cutoff = 0.76
            - if v.late { 0.08 } else { 0.0 }
            - if n <= 2 { 0.09 } else { 0.0 }
            - if wide { 0.045 } else { 0.0 };
        let opens = q >= cutoff
            || pair && (hi >= 6 || n <= 3 || depth >= 45.)
            || suited && hi == 14 && (lo >= 8 || v.late);
        if opens {
            let dead = v.pot.saturating_sub(bb + bb / 2);
            let size = if premium {
                bb.saturating_mul(4)
            } else if strong {
                bb.saturating_mul(3)
            } else {
                bb.saturating_mul(5) / 2
            };
            let wanted = size.saturating_add(dead.min(bb.saturating_mul(4)));
            if let Some((min, max)) = l.raise {
                action = Action::Raise(wanted.clamp(min, max));
                reason = "disciplined_open";
            } else if premium && l.jam.is_some_and(|n| n <= wanted) {
                action = Action::AllIn;
                reason = "disciplined_value_raise";
            }
        }
        if reason == "disciplined_fold"
            && (l.check
                || (premium
                    || odds <= 0.25
                        && depth >= 30.
                        && (pair || suited && (hi >= 12 || hi - lo <= 2)))
                    && l.call.is_some())
        {
            action = cont;
            reason = "disciplined_cheap_flop";
        }
    } else {
        let price = f64::from(call) / f64::from(bb);
        let reraise = premium && (price <= 9. || pair && hi >= 13)
            || pair && hi >= 10 && price <= 3. && (v.late || n <= 2);
        if reraise && let Some((min, max)) = l.raise {
            let wanted = if price >= 8. && depth <= 22. {
                max
            } else {
                target.saturating_mul(if v.late { 3 } else { 4 })
            };
            action = Action::Raise(wanted.clamp(min, max));
            reason = "disciplined_value_raise";
        } else if premium && reraise && l.jam.is_some_and(|n| n <= bb.saturating_mul(12)) {
            action = Action::AllIn;
            reason = "disciplined_value_raise";
        } else if premium && (price <= 12. || pair && hi >= 13 && odds <= 0.52)
            || strong && price <= 4.5 && odds <= 0.34
            || pair && price <= 3.0 && depth >= price * 14.0 && odds < 0.27
            || suited && v.late && q >= 0.76 && price <= 3.0 && odds <= 0.26
        {
            action = cont;
            reason = "disciplined_value_continue";
        }
    }
    Decision {
        action,
        reason,
        equity: None,
        pot_odds: odds,
        samples: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{cards::Card, policy::Legal};

    #[test]
    fn protects_value_and_distinguishes_cheap_from_expensive_speculation() {
        let c = |s| Card::parse(s).unwrap();
        let mut v = View {
            hole: [c("7c"), c("2d")],
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
        assert_eq!(priced(&v, true).action, Action::Fold);
        v.hole = [c("Ah"), c("Kd")];
        assert_eq!(priced(&v, false).action, Action::Raise(80));
        v.hole = [c("2h"), c("2d")];
        v.pot = 370;
        v.legal.call = Some(60);
        v.legal.raise = Some((100, 1000));
        assert_eq!(priced(&v, false).action, Action::Call);
        v.effective = 450;
        assert_eq!(priced(&v, true).action, Action::Fold);
        v.hole = [c("Ah"), c("Ad")];
        v.legal.raise = None;
        v.legal.jam = Some(120);
        assert_eq!(priced(&v, false).action, Action::AllIn);
    }

    #[test]
    fn vetoes_only_learned_expensive_calls_and_preserves_good_prices() {
        let c = |s| Card::parse(s).unwrap();
        let mut v = View {
            hole: [c("Th"), c("9h")],
            board: vec![],
            bb: 20,
            pot: 100,
            bet: 0,
            stack: 745,
            effective: 745,
            late: true,
            ranges: vec![0.; 4],
            legal: Legal {
                fold: true,
                call: Some(70),
                raise: Some((140, 745)),
                ..Default::default()
            },
        };
        let mut learned = Decision {
            action: Action::Call,
            reason: "rollout_preflop",
            equity: None,
            pot_odds: 0.41,
            samples: 0,
        };
        assert_eq!(
            speculative_call_guard(&v, &learned, false),
            Some(Action::Fold)
        );
        learned.reason = "priced_defend";
        assert_eq!(speculative_call_guard(&v, &learned, false), None);
        learned.reason = "rollout_preflop";
        v.pot = 220;
        assert_eq!(speculative_call_guard(&v, &learned, false), None);
        v.hole = [c("Ac"), c("Kd")];
        v.pot = 100;
        assert_eq!(speculative_call_guard(&v, &learned, true), None);
    }
}
