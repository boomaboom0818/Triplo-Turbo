//! Paired, carry-over evaluations with Turbo-owned local opponents only.
use super::*;
use crate::{defense::Defense, policy};

#[derive(Clone, Copy)]
enum Style {
    Tight,
    Pressure,
    Station,
    Elastic,
    Reactive,
    Value,
    Counter,
    Legacy,
    Turbo,
}

fn opponent(
    v: &View,
    style: Style,
    hero_raised: bool,
    frequent: bool,
    rng: &mut ChaCha8Rng,
) -> Action {
    use Style::*;
    let price = v.legal.call.unwrap_or(0);
    match style {
        Tight => policy::with_profile(v, rng.r#gen(), 96, 4).action,
        Value => policy::with_profile(v, rng.r#gen(), 256, 16).action,
        Counter if v.board.is_empty() => {
            let price_bb = f64::from(price) / f64::from(BB);
            let q = policy::quality(v.hole);
            if hero_raised
                && (1.5..=4.0).contains(&price_bb)
                && q >= 0.83
                && let Some((lo, hi)) = v.legal.raise
            {
                Action::Raise((3 * (v.bet + price)).clamp(lo, hi))
            } else {
                crate::preflop::priced(v, true).action
            }
        }
        Counter => policy::with_profile(v, rng.r#gen(), 384, 16).action,
        Legacy => policy::with_profile(v, rng.r#gen(), 384, 14).action,
        Turbo => policy::contextual(v, rng.r#gen(), 384, 22, false, Default::default()).action,
        Pressure => scripted(v, 2, rng),
        Station => scripted(v, 0, rng),
        Elastic | Reactive => {
            if v.board.is_empty()
                && hero_raised
                && (matches!(style, Elastic) || frequent)
                && price > 0
            {
                let [a, b] = v.hole.map(Card::rank);
                let strong = a == b && a >= 7 || a.min(b) >= 10;
                let playable = a == b || a.max(b) >= 11 || v.hole[0].suit() == v.hole[1].suit();
                if strong
                    && price <= 5 * BB
                    && let Some((lo, hi)) = v.legal.raise
                {
                    return Action::Raise(if matches!(style, Elastic) {
                        lo
                    } else {
                        (v.bet + price + v.pot).clamp(lo, hi)
                    });
                }
                if playable && price <= 4 * BB {
                    return Action::Call;
                }
            }
            scripted(v, 1, rng)
        }
    }
}

#[derive(Default)]
struct Record {
    net: i64,
    drawdown: i64,
    busts: usize,
    vetoes: usize,
    early_vetoes: usize,
    premium_vetoes: usize,
    sticky_releases: usize,
    price_vetoes: usize,
    adapted: usize,
    probes: Vec<(usize, i64)>,
}

fn postflop_fork(
    mut h: Hand,
    a: Action,
    hero: usize,
    styles: [Style; 5],
    mut rng: [ChaCha8Rng; 6],
) -> Chips {
    assert!(h.apply(a));
    // The injected bet spends the response budget; no further overrides.
    while let Some(i) = h.actor {
        let v = h.view(i);
        let a = if i == hero {
            policy::contextual(&v, rng[i].r#gen(), 512, 22, true, Default::default()).action
        } else {
            opponent(
                &v,
                styles[(i + 6 - hero - 1) % 6],
                false,
                false,
                &mut rng[i],
            )
        };
        assert!(h.apply(a));
    }
    h.settle()[hero]
}

fn session(
    seed: u64,
    hero: usize,
    length: usize,
    depth: Chips,
    styles: [Style; 5],
    profile: u8,
    probe: bool,
) -> Record {
    let buy = depth * BB;
    let mut stacks = [buy; 6];
    let mut history = Defense::default();
    let mut record = Record::default();
    let mut peak = 0;
    let mut preflop_raises = 0;
    let names = ["seat0", "seat1", "seat2", "seat3", "seat4", "seat5"];
    for t in 0..length {
        let hand_seed = seed + t as u64;
        let mut rng: [_; 6] = std::array::from_fn(|i| {
            ChaCha8Rng::seed_from_u64(hand_seed ^ (i as u64 + 1).wrapping_mul(0x9e3779b97f4a7c15))
        });
        let mut h = Hand::new(6, (hero + t) % 6, stacks, hand_seed);
        history.next_hand();
        let mut spent = false;
        let mut hero_raised = false;
        let mut fork = None;
        // Field uses ONLY public action frequency; the hero never sees styles or this trigger.
        let frequent = t >= 16 && (preflop_raises + 6) * 100 > (t + 24) * 34;
        while let Some(i) = h.actor {
            let v = h.view(i);
            let a = if i == hero {
                let pressure = if h.street == 0 {
                    let opponents = h
                        .active()
                        .filter(|&j| j != hero && h.stack[j] > 0)
                        .map(|j| (names[j], h.tight[j] > 0.));
                    if profile == policy::CONTEXT_PRICE_PROFILE {
                        history.with_calling_evidence(opponents)
                    } else {
                        history.estimate(opponents)
                    }
                } else if policy::uses_postflop_history(profile) {
                    history.postflop_pressure(h.active().filter(|&j| j != hero).map(|j| names[j]))
                } else {
                    Default::default()
                };
                let decision_seed = rng[i].r#gen();
                let d = policy::contextual(&v, decision_seed, 512, profile, spent, pressure);
                if probe
                    && fork.is_none()
                    && d.reason == "guarded_multiway_check"
                    && let Some(bucket) = policy::guard_release_bucket(&v, &d)
                {
                    let alternate = policy::contextual(&v, decision_seed, 512, 24, spent, pressure);
                    assert!(alternate.action != d.action && alternate.spends_response());
                    fork = Some((
                        bucket,
                        postflop_fork(h.clone(), alternate.action, hero, styles, rng.clone()),
                    ));
                }
                spent |= d.spends_response();
                record.vetoes += usize::from(d.reason == "guarded_multiway_check");
                record.early_vetoes += usize::from(d.reason == "guarded_early_steal");
                record.premium_vetoes += usize::from(d.reason == "guarded_premium_raise");
                record.sticky_releases += usize::from(d.reason == "sticky_guard_release");
                record.price_vetoes += usize::from(d.reason == "guarded_preflop_price");
                record.adapted += usize::from(d.reason.starts_with("defended_"));
                d.action
            } else {
                let style = styles[(i + 6 - hero - 1) % 6];
                opponent(&v, style, hero_raised, frequent, &mut rng[i])
            };
            if h.street == 0 || policy::uses_postflop_history(profile) {
                let (name, invested) = match a {
                    Action::Raise(to) => ("raise", to - v.bet),
                    Action::AllIn => ("all_in", v.stack),
                    Action::Call => ("call", v.legal.call.unwrap()),
                    Action::Check => ("check", 0),
                    Action::Fold => ("fold", 0),
                };
                let owing = v.legal.call.unwrap_or(0);
                hero_raised |= h.street == 0
                    && i == hero
                    && matches!(a, Action::Raise(_) | Action::AllIn)
                    && invested > owing;
                if h.street > 0 {
                    history.end_street();
                }
                let street = ["preflop", "flop", "turn", "river"][h.street];
                history.action(&json!({"street":street,"name":names[i],"action":name,"to_call_before":owing,"contribution_delta":invested,"pot_before":h.total.iter().sum::<Chips>()}),Some(hero),i);
            }
            assert!(h.apply(a));
        }
        preflop_raises += usize::from(hero_raised);
        let end = h.settle();
        if let Some((bucket, value)) = fork {
            record
                .probes
                .push((bucket, i64::from(value) - i64::from(end[hero])));
        }
        assert_eq!(end.iter().sum::<Chips>(), stacks.iter().sum::<Chips>());
        record.net += i64::from(end[hero]) - i64::from(stacks[hero]);
        peak = peak.max(record.net);
        record.drawdown = record.drawdown.max(peak - record.net);
        record.busts += usize::from(end[hero] == 0);
        stacks = end.map(|n| if n == 0 { buy } else { n });
    }
    assert_eq!(
        record.net,
        i64::from(stacks[hero]) - i64::from(buy) * (record.busts as i64 + 1)
    );
    record
}

pub fn guard_bench(sessions: usize, length: usize, seed: u64) -> Value {
    use Style::*;
    let fields = [
        ("mixed_50", 50, [Tight, Pressure, Tight, Station, Pressure]),
        (
            "price_sensitive_50",
            50,
            [Elastic, Tight, Station, Elastic, Pressure],
        ),
        (
            "sticky_50",
            50,
            [Station, Station, Tight, Station, Pressure],
        ),
        (
            "reactive_100",
            100,
            [Reactive, Tight, Pressure, Reactive, Station],
        ),
    ];
    let mut output = Vec::new();
    for (f, (field, depth, styles)) in fields.into_iter().enumerate() {
        let seed = seed + f as u64 * 10_000_000;
        let rows: Vec<_> = (0..sessions)
            .into_par_iter()
            .map(|ix| {
                let s = seed + ix as u64 * 1000;
                [policy::GUARDED_ADAPTIVE, policy::ADAPTIVE]
                    .map(|p| session(s, ix % 6, length, depth, styles, p, false))
            })
            .collect();
        let bb100 = |m: i64| 100.0 * m as f64 / (BB as f64 * length as f64);
        let n = sessions as f64;
        let deltas: Vec<_> = rows.iter().map(|[a, b]| bb100(a.net - b.net)).collect();
        let delta = deltas.iter().sum::<f64>() / n;
        let se = (deltas.iter().map(|d| (d - delta).powi(2)).sum::<f64>() / (n - 1.) / n).sqrt();
        let policy_stats = |p: usize| {
            let mut dd: Vec<_> = rows.iter().map(|r| r[p].drawdown).collect();
            dd.sort_unstable();
            json!({"bb100":rows.iter().map(|r|bb100(r[p].net)).sum::<f64>()/n,
                "p90_drawdown_bb":dd[(sessions*9).div_ceil(10)-1] as f64/BB as f64,
                "bust_sessions_pct":100.*rows.iter().filter(|r|r[p].busts>0).count() as f64/n,
                "vetoes":rows.iter().map(|r|r[p].vetoes).sum::<usize>(),"preflop_adaptations":rows.iter().map(|r|r[p].adapted).sum::<usize>()})
        };
        output.push(json!({"field":field,"depth_bb":depth,"seed":seed,"sessions":sessions,"hands_per_policy":sessions*length,
            "profile_22":policy_stats(0),"profile_21":policy_stats(1),"paired_delta_bb100":delta,
            "pointwise_ci95":[delta-1.96*se,delta+1.96*se],"family_ci95":[delta-2.498*se,delta+2.498*se]}));
    }
    json!({"hands_per_session":length,"samples":512,"fields":output})
}

fn lab_fields(holdout: bool) -> Vec<(&'static str, Chips, [Style; 5])> {
    use Style::*;
    let mut fields = vec![
        ("mixed_50", 50, [Tight, Pressure, Tight, Station, Pressure]),
        (
            "price_sensitive_50",
            50,
            [Elastic, Tight, Station, Elastic, Pressure],
        ),
        (
            "calling_heavy_50",
            50,
            [Station, Station, Tight, Station, Pressure],
        ),
        (
            "reactive_100",
            100,
            [Reactive, Tight, Pressure, Reactive, Station],
        ),
        (
            "value_pressure_50",
            50,
            [Value, Elastic, Value, Pressure, Value],
        ),
    ];
    if holdout {
        fields.push((
            "value_and_reactive_50",
            50,
            [Reactive, Value, Elastic, Value, Station],
        ));
    }
    fields
}

pub fn guard_fit(sessions: usize, length: usize, seed: u64) -> Value {
    let mut fields = Vec::new();
    for (i, (field, depth, styles)) in lab_fields(false).into_iter().enumerate() {
        let start = seed + i as u64 * 10_000_000;
        let buckets: Vec<_> = (0..sessions)
            .into_par_iter()
            .map(|r| {
                let mut row = [[0i64; 2]; 8];
                for (bucket, delta) in session(
                    start + r as u64 * 1000,
                    r % 6,
                    length,
                    depth,
                    styles,
                    22,
                    true,
                )
                .probes
                {
                    row[bucket][0] += 1;
                    row[bucket][1] += delta;
                }
                row
            })
            .collect();
        fields.push(json!({"field":field,"seed":start,"sessions":buckets}));
    }
    json!({"bb":BB,"hands_per_session":length,"fields":fields})
}

pub fn lab_bench(sessions: usize, length: usize, seed: u64, candidate: u8) -> Value {
    let fields = lab_fields(candidate != 0);
    let candidates: Vec<u8> = if candidate == 0 {
        vec![23, 24, 25, 26]
    } else {
        vec![candidate]
    };
    compare_sessions(sessions, length, seed, fields, &[21, 22], &candidates)
}

pub fn strategy_bench(sessions: usize, length: usize, seed: u64, candidate: u8) -> Value {
    strategy_comparison(sessions, length, seed, candidate, false)
}

pub fn price_bench(sessions: usize, length: usize, seed: u64, candidate: u8) -> Value {
    strategy_comparison(sessions, length, seed, candidate, true)
}

fn strategy_comparison(
    sessions: usize,
    length: usize,
    seed: u64,
    candidate: u8,
    price: bool,
) -> Value {
    use Style::*;
    let mut fields = vec![
        (
            "value_pressure_50",
            50,
            [Value, Elastic, Value, Pressure, Value],
        ),
        (
            "counter_50",
            50,
            [Counter, Value, Reactive, Counter, Legacy],
        ),
        (
            "mixed_price_50",
            50,
            [Pressure, Elastic, Counter, Tight, Station],
        ),
        (
            "calling_heavy_50",
            50,
            [Station, Station, Tight, Station, Pressure],
        ),
    ];
    if candidate != 0 {
        fields.push((
            "strong_short_50",
            50,
            [Counter, Legacy, Counter, Value, Counter],
        ));
        fields.push((
            "counter_deep_100",
            100,
            [Counter, Value, Reactive, Legacy, Counter],
        ));
        if candidate == policy::CONTEXT_PRICE_PROFILE {
            fields.push(("turbo_pool_50", 50, [Turbo, Counter, Turbo, Legacy, Turbo]));
        }
    }
    let candidates = if candidate == 0 {
        if price {
            vec![32, 33]
        } else {
            vec![29, 30, 31]
        }
    } else {
        vec![candidate]
    };
    compare_sessions(sessions, length, seed, fields, &[22, 28], &candidates)
}

fn compare_sessions(
    sessions: usize,
    length: usize,
    seed: u64,
    fields: Vec<(&'static str, Chips, [Style; 5])>,
    controls: &[u8; 2],
    candidates: &[u8],
) -> Value {
    let policies: Vec<_> = controls
        .iter()
        .copied()
        .chain(candidates.iter().copied())
        .collect();
    // Simultaneous normal intervals, defined by the number of reported comparisons.
    let z = match fields.len() * candidates.len() * controls.len() {
        12 => 2.866,
        14 => 2.914,
        16 => 2.956,
        24 => 3.079,
        40 => 3.227,
        _ => 3.5,
    };
    let mut results = Vec::new();
    let n = sessions as f64;
    let bb100 = |chips: i64| 100. * chips as f64 / (BB as f64 * length as f64);
    for (i, (field, depth, styles)) in fields.into_iter().enumerate() {
        let start = seed + i as u64 * 10_000_000;
        let rows: Vec<Vec<_>> = (0..sessions)
            .into_par_iter()
            .map(|r| {
                let seed = start + r as u64 * 1000;
                policies
                    .iter()
                    .map(|&profile| session(seed, r % 6, length, depth, styles, profile, false))
                    .collect()
            })
            .collect();
        let mut summaries = serde_json::Map::new();
        let mut differences = serde_json::Map::new();
        for (j, id) in policies.iter().enumerate() {
            let mut dd: Vec<_> = rows.iter().map(|r| r[j].drawdown).collect();
            dd.sort_unstable();
            summaries.insert(id.to_string(),json!({"bb100":rows.iter().map(|r|bb100(r[j].net)).sum::<f64>()/n,
                "p90_drawdown_bb":dd[(sessions*9).div_ceil(10)-1] as f64/BB as f64,"bust_sessions_pct":100.*rows.iter().filter(|r|r[j].busts>0).count() as f64/n,
                "postflop_vetoes":rows.iter().map(|r|r[j].vetoes).sum::<usize>(),"preflop_vetoes":rows.iter().map(|r|r[j].early_vetoes).sum::<usize>(),
                "premium_vetoes":rows.iter().map(|r|r[j].premium_vetoes).sum::<usize>(),
                "sticky_releases":rows.iter().map(|r|r[j].sticky_releases).sum::<usize>(),
                "price_vetoes":rows.iter().map(|r|r[j].price_vetoes).sum::<usize>()}));
            if j >= 2 {
                for k in [0, 1] {
                    let deltas: Vec<_> = rows.iter().map(|r| bb100(r[j].net - r[k].net)).collect();
                    let mean = deltas.iter().sum::<f64>() / n;
                    let se =
                        (deltas.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.) / n)
                            .sqrt();
                    differences.insert(format!("{id}-{}",policies[k]),json!({"mean":mean,"ci95":[mean-1.96*se,mean+1.96*se],"family_ci95":[mean-z*se,mean+z*se]}));
                }
            }
        }
        results.push(json!({"field":field,"depth_bb":depth,"seed":start,"policies":summaries,"paired_deltas_bb100":differences}));
    }
    json!({"sessions_per_field":sessions,"hands_per_session":length,"fields":results})
}
