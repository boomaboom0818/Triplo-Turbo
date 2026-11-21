use crate::{
    policy::{Decision, Legal},
    protocol::{State, Turn, features, legal},
};
use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde_json::{Value, json};
use std::{
    env, fs,
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::mpsc,
    time::timeout,
};
use tokio_tungstenite::{
    WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

const WS: &str = "wss://openpoker.ai/ws";
const ACTIVE: &str = "https://api.openpoker.ai/api/me/active-game";
pub const SAMPLES: usize = 512;
pub fn log(event: &str, fields: Value) {
    println!(
        "{}",
        json!({"v":1,"ts_ms":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis(),"event":event,"data":fields})
    );
}
fn key() -> Result<String> {
    let k = if let Ok(f) = env::var("OPENPOKER_KEY_FILE") {
        fs::read_to_string(Path::new(&f)).context("read OPENPOKER_KEY_FILE")?
    } else if let Some(dir) = env::var_os("CREDENTIALS_DIRECTORY") {
        fs::read_to_string(Path::new(&dir).join("openpoker"))
            .context("read systemd openpoker credential")?
    } else {
        env::var("OPENPOKER_API_KEY").context("set OPENPOKER_KEY_FILE or OPENPOKER_API_KEY")?
    };
    let k = k.trim();
    if k.is_empty() {
        bail!("empty Open Poker key");
    }
    Ok(k.into())
}
async fn active(client: &reqwest::Client, k: &str) -> Result<Option<String>> {
    let v = client
        .get(ACTIVE)
        .bearer_auth(k)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    match v["playing"].as_bool() {
        Some(true) => Ok(Some(
            v["table_id"]
                .as_str()
                .context("active seat without table id")?
                .into(),
        )),
        Some(false) => Ok(None),
        None => bail!("invalid active-game response"),
    }
}
pub async fn status() -> Result<()> {
    let c = http()?;
    let key = key()?;
    let table = active(&c, &key).await?;
    let season = c
        .get("https://api.openpoker.ai/api/season/me")
        .bearer_auth(&key)
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    log(
        "account_status",
        json!({"playing":table.is_some(),"table_id":table,"season_hands":season["hands_played"],"season_wins":season["hands_won"],"season_win_rate":season["hands_won"].as_u64().zip(season["hands_played"].as_u64()).filter(|(_,n)|*n>0).map(|(w,n)|w as f64/n as f64),"rank":season["rank"],"score":season["score"]}),
    );
    Ok(())
}
pub async fn performance(limit: usize, ids: Option<&String>) -> Result<()> {
    use std::collections::HashSet;
    let client = http()?;
    let key = key()?;
    let mut seen = HashSet::new();
    let mut hands = Vec::new();
    let mut offset = 0;
    let mut skipped = 0;
    let filter = ids.map(fs::read_to_string).transpose()?.map(|s| {
        s.lines()
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_owned)
            .collect::<HashSet<_>>()
    });
    while seen.len() < limit && !filter.as_ref().is_some_and(|w| hands.len() == w.len()) {
        if offset > 0 {
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
        let page = (limit - seen.len()).min(200);
        let obj = client
            .get("https://api.openpoker.ai/api/me/hand-history")
            .query(&[("limit", page), ("offset", offset)])
            .bearer_auth(&key)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        let rows = obj["hands"]
            .as_array()
            .context("invalid hand-history response")?;
        offset += rows.len();
        let before = seen.len();
        for row in rows {
            let Some(id) = row["hand_id"].as_str() else {
                skipped += 1;
                continue;
            };
            if !seen.insert(id.to_owned()) {
                continue;
            }
            if filter.as_ref().is_some_and(|w| !w.contains(id)) {
                continue;
            }
            let delta = row["stack_end"]
                .as_i64()
                .zip(row["stack_start"].as_i64())
                .and_then(|(end, start)| end.checked_sub(start));
            let profit = row["profit"].as_i64();
            if profit.zip(delta).is_some_and(|(p, d)| p != d) {
                skipped += 1;
                continue;
            }
            let net = delta.or(profit);
            let bb = row["big_blind"]
                .as_u64()
                .or_else(|| row["blinds"]["big_blind"].as_u64())
                .unwrap_or(20);
            if let Some(net) = net.filter(|_| bb > 0) {
                hands.push((
                    row["started_at"].as_str().unwrap_or("").to_owned(),
                    net as f64 / bb as f64,
                ));
            } else {
                skipped += 1;
            }
        }
        if rows.len() < page || seen.len() == before {
            break;
        }
    }
    hands.reverse();
    hands.sort_by(|a, b| a.0.cmp(&b.0));
    let mut r = crate::stats::performance(&hands.iter().map(|h| h.1).collect::<Vec<_>>());
    r["scope"] = if let Some(wanted) = filter {
        r["requested_hand_ids"] = json!(wanted.len());
        r["unmatched_hand_ids"] = json!(wanted.len() - hands.len());
        json!("explicit hand IDs matched against account history")
    } else {
        json!("recent account hands; may include multiple policy versions")
    };
    r["skipped_invalid"] = json!(skipped);
    r["first"] = json!(hands.first().map(|h| &h.0));
    r["last"] = json!(hands.last().map(|h| &h.0));
    println!("{}", serde_json::to_string_pretty(&r)?);
    Ok(())
}
fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

#[derive(Default)]
struct Memory {
    state: State,
    sent: Option<(Turn, Value)>,
    acked: bool,
    fallback_for: Option<Turn>,
    attempts: u8,
    override_hand: String,
    unknown_prior_play: bool,
    queue_at: Option<Instant>,
}

impl Memory {
    fn reset_table(&mut self) {
        let hud = self.state.take_hud();
        *self = Self::default();
        self.state.set_hud(hud);
    }

    fn reconcile_seat(&mut self, actual: Option<String>) {
        match actual {
            Some(id) if id != self.state.table => {
                self.reset_table();
                self.state.table(&id);
                self.unknown_prior_play = true;
            }
            None if !self.state.table.is_empty() => self.reset_table(),
            _ => (),
        }
    }
}

fn delayed_rejoin(m: &Value) -> Instant {
    let explicit = m["cooldown_seconds"]
        .as_u64()
        .or_else(|| m["retry_after"].as_u64())
        .or_else(|| m["details"]["retry_after"].as_u64());
    let wall_clock = m["rebuy_at"].as_str().and_then(|s| {
        // Server timestamp may be RFC3339. Keep the documented free cooldown
        // if there is no numeric hint; confirmation can wake us up sooner.
        s.parse::<u64>().ok().map(|epoch| {
            epoch.saturating_sub(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            )
        })
    });
    Instant::now() + Duration::from_secs(explicit.or(wall_clock).unwrap_or(300).clamp(1, 600))
}
fn recover(s: &State) -> Value {
    json!({"type":"resync_request","table_id":s.table,"last_table_seq":s.seq})
}
async fn send<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    m: Value,
) -> Result<()> {
    timeout(
        Duration::from_secs(4),
        ws.send(Message::text(m.to_string())),
    )
    .await??;
    Ok(())
}
struct Work {
    turn: Turn,
    result: Decision,
    info: Value,
    t: Instant,
    seed: u64,
}
struct Job {
    turn: Turn,
    menu: Legal,
    started: Instant,
}

async fn session<S: AsyncRead + AsyncWrite + Unpin>(
    ws: &mut WebSocketStream<S>,
    mem: &mut Memory,
    buy: u32,
    profile: u8,
) -> Result<()> {
    let mut job: Option<Job> = None;
    let (tx, mut rx) = mpsc::channel::<Work>(4);
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_in = Instant::now();
    let mut last_ping = last_in;
    let mut syncing = false;
    let mut recovery_since = None;
    let mut pending_rebuy = false;
    let join = || json!({"type":"join_lobby","buy_in":buy});
    loop {
        let mut ready: Option<Work> = None;
        enum Event {
            Frame(Option<std::result::Result<Message, tokio_tungstenite::tungstenite::Error>>),
            Done(Work),
            Tick,
        }
        let event = tokio::select! {biased; f=ws.next()=>Event::Frame(f), Some(w)=rx.recv()=>Event::Done(w), _=tick.tick()=>Event::Tick};
        match event {
            Event::Frame(frame) => {
                let msg = frame.context("socket ended")??;
                last_in = Instant::now();
                let Message::Text(text) = msg else {
                    if matches!(msg, Message::Close(_)) {
                        bail!("socket closed");
                    }
                    if matches!(msg, Message::Ping(_)) {
                        ws.flush().await?;
                    }
                    continue;
                };
                let m: Value = serde_json::from_str(&text).context("invalid server JSON")?;
                let kind = m["type"].as_str().unwrap_or("");
                let state = &mut mem.state;
                if let Some(id) = m["table_id"].as_str() {
                    if kind == "table_joined" {
                        state.table(id);
                    } else if id != state.table
                        && !matches!(kind, "error" | "rebuy_confirmed" | "auto_rebuy_scheduled")
                    {
                        continue;
                    }
                }
                let seq = m["table_seq"].as_u64();
                if kind != "resync_response" && seq.is_some_and(|s| s <= state.seq) {
                    continue;
                }
                if kind != "resync_response" {
                    if let Some(s) = seq {
                        state.seq = s;
                    }
                    state.observe(&m);
                }
                let mut authority = None;
                match kind {
                    "connected" => {
                        log("connected", json!({"agent_id":m["agent_id"]}));
                        if state.table.is_empty() {
                            if mem.queue_at.is_none_or(|when| when <= Instant::now()) {
                                mem.queue_at = None;
                                send(ws, join()).await?;
                            }
                        } else {
                            send(ws, recover(state)).await?;
                            syncing = true;
                            recovery_since = Some(Instant::now());
                        }
                    }
                    "lobby_joined" => {
                        pending_rebuy = false;
                        mem.queue_at = None;
                        log("queued", json!({"position":m["position"]}));
                        send(ws, json!({"type":"set_auto_rebuy","enabled":true})).await?;
                    }
                    "table_joined" => {
                        pending_rebuy = false;
                        mem.queue_at = None;
                        log("seated", json!({"table":state.table,"seat":state.hero}));
                    }
                    "your_turn" if !syncing => {
                        authority = Turn::read(&m["hand_id"], &m["turn_token"])
                            .map(|t| (t, legal(&m["valid_actions"]), false));
                    }
                    "resync_response" => {
                        if m["role"] == "spectator" || m["snapshot"]["type"] != "table_state" {
                            bail!("invalid player resync");
                        }
                        if m["to_table_seq"].as_u64().is_some_and(|s| s < state.seq) {
                            if syncing {
                                bail!("resync watermark regressed");
                            }
                            continue;
                        }
                        let cold = state.hand.is_empty();
                        state.restore(&m);
                        if cold || mem.unknown_prior_play {
                            mem.override_hand = state.hand.clone();
                            mem.unknown_prior_play = false;
                        }
                        syncing = false;
                        recovery_since = None;
                        job = None;
                        let s = &m["snapshot"];
                        let h = &s["hero"];
                        if h["seat"].is_u64() && h["seat"] == s["actor_seat"] {
                            authority = Turn::read(&json!(state.hand), &h["turn_token"])
                                .map(|t| (t, legal(&h["valid_actions"]), true));
                        }
                    }
                    "table_state" => {
                        if m["actor_seat"]
                            .as_u64()
                            .is_none_or(|s| Some(s as usize) != state.hero)
                            || job.as_ref().is_some_and(|j| j.turn.hand != state.hand)
                        {
                            job = None;
                        }
                    }
                    "action_ack" => {
                        if mem
                            .sent
                            .as_ref()
                            .is_some_and(|(_, v)| v["client_action_id"] == m["client_action_id"])
                        {
                            mem.acked = true;
                        }
                        log("ack", json!({"id":m["client_action_id"]}));
                    }
                    "player_action" => {
                        if m["seat"].as_u64().map(|x| x as usize) == state.hero {
                            job = None;
                        }
                        log(
                            "played",
                            json!({"hand":state.hand,"seat":m["seat"],"street":m["street"],"action":m["action"],"amount":m["amount"],"pot":m["pot"]}),
                        );
                    }
                    "hand_result" | "hand_start" => {
                        job = None;
                        mem.sent = None;
                        mem.attempts = 0;
                        mem.fallback_for = None;
                        log(
                            kind,
                            json!({"hand":state.hand,"table":state.table,"seat":state.hero,"button":m["dealer_seat"],"total_pot":m["total_pot"],"final_stacks":m["final_stacks"],"payouts":m["payouts"],"winner_seats":m["winners"].as_array().map(|w|w.iter().filter_map(|p|p["seat"].as_u64()).collect::<Vec<_>>())}),
                        );
                    }
                    "action_rejected" => {
                        job = None;
                        log("rejected", json!({"code":m["code"],"hand":state.hand}));
                        if let Some((turn, _)) = mem.sent.take() {
                            mem.fallback_for = Some(turn);
                        }
                        if !state.table.is_empty() && !syncing {
                            send(ws, recover(state)).await?;
                            syncing = true;
                            recovery_since = Some(Instant::now());
                        }
                    }
                    "table_closed" | "season_ended" | "rebuy_confirmed" => {
                        job = None;
                        mem.reset_table();
                        syncing = false;
                        recovery_since = None;
                        pending_rebuy = false;
                        log(kind, json!({}));
                        let bal = m["chip_balance"].as_u64().unwrap_or(buy as u64);
                        let b = (buy as u64).min(bal).max(1000);
                        send(ws, json!({"type":"join_lobby","buy_in":b})).await?;
                    }
                    "busted" => {
                        log("busted", json!({}));
                        // The table is gone for this player. Give cash-out / server
                        // auto-rebuy a moment, then attempt a normal buy-in first.
                        job = None;
                        mem.reset_table();
                        mem.queue_at = Some(Instant::now() + Duration::from_secs(1));
                        syncing = false;
                        recovery_since = None;
                        pending_rebuy = false;
                    }
                    "auto_rebuy_scheduled" => {
                        log(kind, json!({"cooldown_seconds":m["cooldown_seconds"]}));
                        if state.table.is_empty() {
                            pending_rebuy = false;
                            mem.queue_at = Some(delayed_rejoin(&m));
                        }
                    }
                    "error" => {
                        let code = m["code"].as_str().unwrap_or("unknown");
                        log("server_error", json!({"code":code}));
                        match code {
                            "already_seated" => {
                                let id = m["table_id"]
                                    .as_str()
                                    .or_else(|| m["details"]["table_id"].as_str())
                                    .context("already seated, table unknown")?;
                                state.table(id);
                                if !syncing {
                                    send(ws, recover(state)).await?;
                                    syncing = true;
                                    recovery_since = Some(Instant::now());
                                }
                            }
                            "not_at_table" | "table_not_found" => {
                                job = None;
                                mem.reset_table();
                                mem.queue_at = Some(Instant::now() + Duration::from_secs(1));
                                syncing = false;
                                recovery_since = None;
                            }
                            "insufficient_season_chips"
                                if state.table.is_empty() && !pending_rebuy =>
                            {
                                pending_rebuy = true;
                                send(ws, json!({"type":"rebuy","amount":0})).await?;
                                mem.queue_at = Some(Instant::now() + Duration::from_secs(30));
                            }
                            "rebuy_on_cooldown" if state.table.is_empty() => {
                                pending_rebuy = false;
                                mem.queue_at = Some(delayed_rejoin(&m));
                            }
                            "cannot_rebuy" if state.table.is_empty() && pending_rebuy => {
                                // A server-side auto-rebuy or cash-out may have won the race.
                                pending_rebuy = false;
                                mem.queue_at = Some(Instant::now() + Duration::from_secs(12));
                            }
                            "already_in_lobby" => {
                                pending_rebuy = false;
                                mem.queue_at = None;
                            }
                            _ => bail!("Open Poker error: {code}"),
                        }
                    }
                    _ => (),
                }
                if let Some((turn, l, restored)) = authority {
                    if job.as_ref().is_some_and(|j| j.turn == turn) {
                        continue;
                    }
                    if let Some((_, payload)) = mem.sent.as_ref().filter(|(t, _)| *t == turn) {
                        if restored && !mem.acked {
                            send(ws, payload.clone()).await?;
                        }
                        continue;
                    }
                    let fallback = l.passive().context("empty server legal menu")?;
                    let state = &mem.state;
                    if mem.fallback_for.as_ref() != Some(&turn) {
                        mem.attempts = 0;
                    }
                    if mem.attempts >= 2 {
                        continue;
                    }
                    let view = state.view(l);
                    if view.is_none() && !restored && !state.table.is_empty() {
                        job = None;
                        send(ws, recover(state)).await?;
                        syncing = true;
                        recovery_since = Some(Instant::now());
                        continue;
                    }
                    let seed = rand::thread_rng().r#gen();
                    let t = Instant::now();
                    job = Some(Job {
                        turn: turn.clone(),
                        menu: l,
                        started: t,
                    });
                    if mem.fallback_for.as_ref() == Some(&turn) || view.is_none() {
                        ready = Some(Work {
                            turn,
                            result: Decision {
                                action: fallback,
                                reason: "validated_fallback",
                                equity: None,
                                pot_odds: 0.0,
                                samples: 0,
                            },
                            info: json!({"legal":l}),
                            seed,
                            t,
                        });
                    } else if let Some(v) = view {
                        let tx = tx.clone();
                        let used = mem.override_hand == turn.hand;
                        if profile == crate::v2::V2_PROFILE
                            && let Some(ctx) = state.v2_context(l, state)
                        {
                            let mut info = features(&v);
                            let opp_meta: Vec<Value> = ctx
                                .opponents
                                .iter()
                                .map(|o| {
                                    json!({
                                        "s": o.seat,
                                        "pos": format!("{:?}", o.pos),
                                        "pf": format!("{:?}", o.preflop_line),
                                        "aggr": o.is_aggressor,
                                        "h": o.hud.hands.round() as u32,
                                        "vpip": (o.hud.vpip * 100.0).round() as u32,
                                        "f2b": (o.hud.fold_to_bet * 100.0).round() as u32,
                                    })
                                })
                                .collect();
                            info["v2"] = json!({
                                "pos": format!("{:?}", ctx.pos),
                                "ip": ctx.in_position,
                                "pfa": ctx.hero_was_pfa,
                                "pf_r": ctx.preflop_raises,
                                "st_r": ctx.street_raises,
                                "fcr": ctx.facing_check_raise,
                                "opps": opp_meta,
                            });
                            tokio::task::spawn_blocking(move || {
                                let result = crate::v2::decide_v2(&ctx, seed, SAMPLES);
                                let _ = tx.blocking_send(Work {
                                    turn,
                                    result,
                                    info,
                                    seed,
                                    t,
                                });
                            });
                        } else {
                            let pressure = if crate::policy::uses_history(profile) && v.board.is_empty()
                            {
                                if profile == crate::policy::CONTEXT_PRICE_PROFILE {
                                    state.priced_pressure_from(state)
                                } else {
                                    state.pressure_from(state)
                                }
                            } else if crate::policy::uses_postflop_history(profile)
                                && !v.board.is_empty()
                            {
                                state.postflop_pressure_from(state)
                            } else {
                                Default::default()
                            };
                            tokio::task::spawn_blocking(move || {
                                let result = crate::policy::contextual(
                                    &v, seed, SAMPLES, profile, used, pressure,
                                );
                                let _ = tx.blocking_send(Work {
                                    turn,
                                    result,
                                    info: features(&v),
                                    seed,
                                    t,
                                });
                            });
                        }
                    }
                }
            }
            Event::Done(w) => {
                ready = Some(w);
            }
            Event::Tick => {
                if last_in.elapsed() > Duration::from_secs(70)
                    || recovery_since
                        .is_some_and(|t: Instant| t.elapsed() > Duration::from_secs(10))
                {
                    bail!("socket or recovery timed out");
                }
                if last_ping.elapsed() > Duration::from_secs(25) {
                    ws.send(Message::Ping(vec![].into())).await?;
                    last_ping = Instant::now();
                }
                if mem.state.table.is_empty()
                    && mem.queue_at.is_some_and(|when| when <= Instant::now())
                {
                    mem.queue_at = None;
                    pending_rebuy = false;
                    send(ws, join()).await?;
                }
                if let Some(j) = job
                    .as_ref()
                    .filter(|j| j.started.elapsed() > Duration::from_millis(500))
                {
                    ready = Some(Work {
                        turn: j.turn.clone(),
                        result: Decision {
                            action: j.menu.passive().unwrap(),
                            reason: "compute_timeout",
                            equity: None,
                            pot_odds: 0.0,
                            samples: 0,
                        },
                        info: json!({"legal":j.menu}),
                        seed: 0,
                        t: j.started,
                    });
                }
            }
        }
        if let Some(w) = ready
            && job
                .as_ref()
                .is_some_and(|j| j.turn == w.turn && j.menu.permits(w.result.action))
        {
            job = None;
            let id = format!("{:032x}", rand::thread_rng().r#gen::<u128>());
            let payload = w.turn.payload(w.result.action, &id);
            if w.result.spends_response() {
                mem.override_hand = w.turn.hand.clone();
            }
            mem.sent = Some((w.turn.clone(), payload.clone()));
            mem.acked = false;
            mem.attempts += 1;
            send(ws, payload).await?;
            log(
                "decision",
                json!({"hand":w.turn.hand,"policy":"turbo-v1","profile":profile,"id":id,"seed":w.seed,"decision_us":w.t.elapsed().as_micros(),"decision":w.result,"view":w.info}),
            );
        }
    }
}

async fn season_balance(client: &reqwest::Client, k: &str) -> Option<u32> {
    let v = client
        .get("https://api.openpoker.ai/api/season/me")
        .bearer_auth(k)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    v["chip_balance"].as_u64().and_then(|x| u32::try_from(x).ok())
}

pub async fn run(buy: u32, profile: u8) -> Result<()> {
    let k = key()?;
    let client = http()?;
    let mut mem = Memory::default();
    let mut backoff = 2;
    loop {
        let start = Instant::now();
        let outcome:Result<()>=async {
        mem.reconcile_seat(active(&client,&k).await?);
        let eff_buy = season_balance(&client, &k)
            .await
            .map(|bal| buy.min(bal).max(1000))
            .unwrap_or(buy);
        let mut req=WS.into_client_request()?;let mut h=format!("Bearer {k}").parse::<tokio_tungstenite::tungstenite::http::HeaderValue>().map_err(|_|anyhow!("invalid key encoding"))?;h.set_sensitive(true);req.headers_mut().insert("Authorization",h);
        let (mut ws,_)=timeout(Duration::from_secs(10),connect_async(req)).await??;
        tokio::select!{r=session(&mut ws,&mut mem,eff_buy,profile)=>r,_=shutdown()=>{let _=send(&mut ws,json!({"type":"leave_table"})).await;Ok(())}}
    }.await;
        match outcome {
            Ok(()) => return Ok(()),
            Err(e) => {
                let reason = e.to_string();
                log("disconnected", json!({"reason":reason,"retry_s":backoff}));
                if reason.contains("401")
                    || reason.contains("403")
                    || reason.contains("auth_failed")
                {
                    bail!("Open Poker authentication failed");
                }
            }
        }
        let jitter = rand::thread_rng().gen_range(0..900);
        tokio::select! {_=shutdown()=>return Ok(()),_=tokio::time::sleep(Duration::from_millis(1000*backoff+jitter))=>()};
        backoff = if start.elapsed() > Duration::from_secs(60) {
            2
        } else {
            (backoff * 2).min(30)
        };
    }
}
async fn shutdown() {
    #[cfg(unix)]
    {
        let mut sig = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {_=sig.recv()=>(),_=tokio::signal::ctrl_c()=>()}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::{MaybeTlsStream, accept_async};
    async fn next<S: AsyncRead + AsyncWrite + Unpin>(ws: &mut WebSocketStream<S>) -> Value {
        let s = timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_text()
            .unwrap();
        serde_json::from_str(&s).unwrap()
    }
    async fn pair() -> (
        WebSocketStream<MaybeTlsStream<TcpStream>>,
        WebSocketStream<TcpStream>,
    ) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let a = l.local_addr().unwrap();
        let (c, s) = tokio::join!(connect_async(format!("ws://{a}")), async {
            accept_async(l.accept().await.unwrap().0).await.unwrap()
        });
        (c.unwrap().0, s)
    }
    fn snapshot(seq: u64) -> Value {
        json!({"type":"table_state","table_id":"t","hand_id":"h","table_seq":seq,"dealer_seat":0,"big_blind":20,"street":"flop","pot":120,"actor_seat":0,"board":["Ah","Ad","2c"],"seats":[{"seat":0,"stack":900,"bet":0,"in_hand":true},{"seat":1,"stack":900,"bet":20,"in_hand":true}],"hero":{"seat":0,"hole_cards":["Ac","As"],"turn_token":"test-turn","valid_actions":[{"action":"fold"},{"action":"call","amount":20},{"action":"raise","min":47,"max":60}]}})
    }
    fn resync(seq: u64) -> Value {
        json!({"type":"resync_response","role":"player","table_id":"t","to_table_seq":seq,"replayed_events":[],"snapshot":snapshot(seq)})
    }
    #[tokio::test]
    async fn real_socket_lobby_reconnect_retry_and_rejection() {
        let mut mem = Memory::default();
        let (mut c, mut s) = pair().await;
        let server = async {
            send(&mut s, json!({"type":"connected","agent_id":"a"}))
                .await
                .unwrap();
            assert_eq!(next(&mut s).await["type"], "join_lobby");
            send(&mut s, json!({"type":"lobby_joined"})).await.unwrap();
            assert_eq!(next(&mut s).await["type"], "set_auto_rebuy");
            send(
                &mut s,
                json!({"type":"table_joined","table_id":"t","seat":0}),
            )
            .await
            .unwrap();
            send(&mut s, snapshot(8)).await.unwrap();
            // Even an ordinary snapshot containing legal actions and a token cannot authorize a send.
            assert!(timeout(Duration::from_millis(30), s.next()).await.is_err());
            let snap = snapshot(8);
            send(&mut s,json!({"type":"your_turn","table_seq":12,"hand_id":"h","turn_token":"test-turn","community_cards":snap["board"],"pot":120,"valid_actions":snap["hero"]["valid_actions"]})).await.unwrap();
            let a = next(&mut s).await;
            assert_eq!(a["action"], "raise");
            assert_eq!(a["amount"], 60);
            assert_eq!(a["hand_id"], "h");
            assert_eq!(a["turn_token"], "test-turn");
            s.close(None).await.unwrap();
            a
        };
        let (result, original) = tokio::join!(session(&mut c, &mut mem, 2000, 20), server);
        assert!(result.is_err());
        let (mut c, mut s) = pair().await;
        let server = async {
            send(&mut s, json!({"type":"connected"})).await.unwrap();
            let r = next(&mut s).await;
            assert_eq!(r["type"], "resync_request");
            assert_eq!(r["last_table_seq"], 12);
            send(&mut s, resync(17)).await.unwrap();
            assert_eq!(next(&mut s).await, original);
            send(
                &mut s,
                json!({"type":"action_rejected","code":"invalid_action"}),
            )
            .await
            .unwrap();
            assert_eq!(next(&mut s).await["type"], "resync_request");
            send(&mut s, resync(20)).await.unwrap();
            let f = next(&mut s).await;
            assert_eq!(f["action"], "fold");
            assert_ne!(f["client_action_id"], original["client_action_id"]);
            assert!(f.get("amount").is_none());
            send(
                &mut s,
                json!({"type":"action_ack","client_action_id":f["client_action_id"]}),
            )
            .await
            .unwrap();
            send(&mut s, resync(24)).await.unwrap();
            assert!(timeout(Duration::from_millis(30), s.next()).await.is_err());
            s.close(None).await.unwrap();
        };
        let _ = tokio::join!(session(&mut c, &mut mem, 2000, 20), server);
    }

    #[tokio::test]
    async fn busted_requeues_without_rebuying_until_lobby_reports_low_balance() {
        let mut mem = Memory::default();
        mem.reconcile_seat(Some("t".into()));
        let (mut c, mut s) = pair().await;
        let server = async {
            send(&mut s, json!({"type":"connected"})).await.unwrap();
            assert_eq!(next(&mut s).await["type"], "resync_request");
            let mut idle = resync(21);
            idle["snapshot"]["actor_seat"] = json!(1);
            send(&mut s, idle).await.unwrap();
            send(&mut s, json!({"type":"busted","table_id":"t"}))
                .await
                .unwrap();
            // Some broadcasts are in flight when the seat is removed.
            send(&mut s, snapshot(31)).await.unwrap();
            assert_eq!(
                next(&mut s).await,
                json!({"type":"join_lobby","buy_in":1000})
            );
            send(
                &mut s,
                json!({"type":"error","code":"insufficient_season_chips"}),
            )
            .await
            .unwrap();
            assert_eq!(next(&mut s).await, json!({"type":"rebuy","amount":0}));
            send(
                &mut s,
                json!({"type":"error","code":"rebuy_on_cooldown","retry_after":1}),
            )
            .await
            .unwrap();
            assert_eq!(
                next(&mut s).await,
                json!({"type":"join_lobby","buy_in":1000})
            );
            send(&mut s, json!({"type":"lobby_joined"})).await.unwrap();
            assert_eq!(next(&mut s).await["type"], "set_auto_rebuy");
            s.close(None).await.unwrap();
        };
        let _ = tokio::join!(session(&mut c, &mut mem, 1000, 22), server);
        assert!(mem.state.table.is_empty());
        assert!(mem.queue_at.is_none());
    }

    #[test]
    fn active_game_clears_stale_but_preserves_confirmed_table_history() {
        let mut mem = Memory::default();
        mem.reconcile_seat(Some("first".into()));
        mem.state.seq = 42;
        mem.reconcile_seat(Some("first".into()));
        assert_eq!(mem.state.seq, 42);
        mem.reconcile_seat(Some("other".into()));
        assert_eq!(mem.state.table, "other");
        assert_eq!(mem.state.seq, 0);
        mem.reconcile_seat(None);
        assert!(mem.state.table.is_empty());
    }
    #[test]
    fn malformed_cards_and_menus_do_not_manufacture_actions() {
        let mut s = State::default();
        let mut snap = snapshot(1);
        snap["hero"]["hole_cards"] = json!(["Ac", "Ah"]);
        s.observe(&snap);
        let l = legal(&json!([{"action":"call","amount":8},{"action":"raise","min":55,"max":45}]));
        assert!(s.view(l).is_none());
        assert_eq!(l.passive(), Some(crate::policy::Action::Call));
        assert_eq!(l.raise, None);
    }
}
