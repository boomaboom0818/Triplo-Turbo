//! Replay only our own player's redacted wire capture. No credentials or socket.
use crate::{
    live::{SAMPLES, log},
    protocol::{State, features, legal},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
    time::{Duration, Instant},
};

pub fn stdio() -> Result<()> {
    use std::io::Write;
    let mut history = State::default();
    for line in std::io::stdin().lock().lines() {
        let r: Value = serde_json::from_str(&line?)?;
        let mut s = State::default();
        for e in r["events"].as_array().into_iter().flatten() {
            s.observe(e);
        }
        let snap = &r["snapshot"];
        s.observe(snap);
        let l = legal(&snap["hero"]["valid_actions"]);
        anyhow::ensure!(l.passive().is_some(), "empty legal menu");
        let v = s.view(l).context("invalid offline view")?;
        let p = r["profile"]
            .as_u64()
            .unwrap_or(crate::policy::DEFAULT_PROFILE as u64);
        anyhow::ensure!(
            p <= crate::policy::MAX_LAB_PROFILE as u64 || p == crate::v2::V2_PROFILE as u64,
            "invalid profile"
        );
        let d = if p as u8 == crate::v2::V2_PROFILE {
            if let Some(session) = r["history_session"].as_str().filter(|s| !s.is_empty()) {
                history.table(session);
                for hand in r["completed_hands"].as_array().into_iter().flatten() {
                    for e in hand.as_array().context("invalid hand history")? {
                        history.observe(e);
                    }
                }
                history.observe(&json!({"type":"offline_boundary","hand_id":snap["hand_id"]}));
            }
            let ctx = s.v2_context(l, &history).context("invalid v2 view")?;
            crate::v2::decide_v2(&ctx, r["seed"].as_u64().context("missing seed")?, SAMPLES)
        } else {
            let uses_history = crate::policy::uses_history(p as u8);
            let mut pressure = Default::default();
            if uses_history
                && let Some(session) = r["history_session"].as_str().filter(|s| !s.is_empty())
            {
                history.table(session);
                for hand in r["completed_hands"].as_array().into_iter().flatten() {
                    for e in hand.as_array().context("invalid hand history")? {
                        history.observe(e);
                    }
                }
                history.observe(&json!({"type":"offline_boundary","hand_id":snap["hand_id"]}));
                if v.board.is_empty() {
                    pressure = if p as u8 == crate::policy::CONTEXT_PRICE_PROFILE {
                        s.priced_pressure_from(&history)
                    } else {
                        s.pressure_from(&history)
                    };
                } else if crate::policy::uses_postflop_history(p as u8) {
                    pressure = s.postflop_pressure_from(&history);
                }
            }
            crate::policy::contextual(
                &v,
                r["seed"].as_u64().context("missing seed")?,
                SAMPLES,
                if r["steal_probe"] == true {
                    20
                } else {
                    p as u8
                },
                r["response_used"] == true,
                pressure,
            )
        };
        let uses_history = crate::policy::uses_history(p as u8);
        let mut out = serde_json::to_value(&d)?;
        if uses_history {
            out["spends_response"] = json!(d.spends_response());
            if r["steal_probe"] == true && r["response_used"] != true {
                let pressure = s.pressure_from(&history);
                out["steal_options"] = json!(crate::policy::steal_options(&v, d.action, pressure));
            }
        }
        if r["probe"] == true {
            out["probe"] = json!(crate::policy::response_probe(&v, &d));
        }
        println!("{}", out);
        std::io::stdout().flush()?;
    }
    Ok(())
}
pub fn run(path: &Path, follow: bool, profile: u8) -> Result<()> {
    let f = File::open(path).context("open player wire JSONL")?;
    let mut reader = BufReader::new(f);
    if follow {
        reader.seek(SeekFrom::End(0))?;
    }
    let mut state = State::default();
    let mut override_hand = String::new();
    let (mut turns, mut missing, mut max_us, mut postflop, mut adaptations) = (0, 0, 0, 0, 0);
    let mut line = String::new();
    loop {
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            if !follow {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let new = std::fs::metadata(path)?;
                if new.ino() != reader.get_ref().metadata()?.ino()
                    || new.len() < reader.stream_position()?
                {
                    reader = BufReader::new(File::open(path)?);
                    state = State::default();
                    line.clear();
                }
            }
            continue;
        }
        if !line.ends_with('\n') && follow {
            continue;
        }
        let row: Value = serde_json::from_str(&line).context("invalid JSONL capture")?;
        line.clear();
        let m = row.get("message").unwrap_or(&row);
        if !m.is_object() {
            continue;
        }
        if let Some(id) = m["table_id"].as_str() {
            state.table(id);
        }
        let seq = m["table_seq"].as_u64();
        if seq.is_some_and(|s| s <= state.seq) {
            continue;
        }
        let menu = match m["type"].as_str() {
            Some("resync_response") => {
                state.restore(m);
                let snap = &m["snapshot"];
                if snap["hero"]["seat"].is_u64() && snap["actor_seat"] == snap["hero"]["seat"] {
                    Some(legal(&snap["hero"]["valid_actions"]))
                } else {
                    None
                }
            }
            _ => {
                state.observe(m);
                if let Some(s) = seq {
                    state.seq = s;
                }
                if m["type"] == "your_turn" {
                    Some(legal(&m["valid_actions"]))
                } else {
                    None
                }
            }
        };
        if let Some(l) = menu
            && l.passive().is_some()
        {
            let Some(v) = state.view(l) else {
                missing += 1;
                continue;
            };
            let seed = state.seq;
            let t = Instant::now();
            let d = if profile == crate::v2::V2_PROFILE
                && let Some(ctx) = state.v2_context(l, &state)
            {
                crate::v2::decide_v2(&ctx, seed, SAMPLES)
            } else {
                let pressure = if crate::policy::uses_history(profile) && v.board.is_empty() {
                    if profile == crate::policy::CONTEXT_PRICE_PROFILE {
                        state.priced_pressure_from(&state)
                    } else {
                        state.pressure_from(&state)
                    }
                } else if crate::policy::uses_postflop_history(profile) {
                    state.postflop_pressure_from(&state)
                } else {
                    Default::default()
                };
                crate::policy::contextual(
                    &v,
                    seed,
                    SAMPLES,
                    profile,
                    override_hand == state.hand,
                    pressure,
                )
            };
            adaptations += usize::from(d.reason.starts_with("defended_"));
            if d.spends_response() {
                override_hand = state.hand.clone();
            }
            let us = t.elapsed().as_micros();
            max_us = max_us.max(us);
            anyhow::ensure!(l.permits(d.action), "policy chose illegal action");
            turns += 1;
            if !v.board.is_empty() {
                postflop += 1;
            }
            if follow {
                log(
                    "shadow_decision",
                    json!({"hand":state.hand,"table_seq":state.seq,"decision":d,"seed":seed,"latency_us":us,"view":features(&v)}),
                );
            }
        }
    }
    log(
        "replay_summary",
        json!({"legal_decisions":turns,"profile":profile,"adaptations":adaptations,"postflop":postflop,"missing_view":missing,"max_decision_us":max_us,"samples":SAMPLES}),
    );
    Ok(())
}
