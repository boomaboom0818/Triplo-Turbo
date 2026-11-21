mod arena;
mod cards;
mod defense;
mod eval;
mod hud;
mod live;
mod policy;
mod preflop;
mod protocol;
mod range;
mod shadow;
mod stats;
mod v2;
use anyhow::{Context, Result, bail, ensure};
use std::path::Path;

fn arg<T: std::str::FromStr>(args: &[String], i: usize, d: T) -> Result<T> {
    args.get(i)
        .map(|s| s.parse().ok().context("invalid numeric argument"))
        .unwrap_or(Ok(d))
}
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("live") if a.len() <= 3 => {
            let b = arg(&a, 1, 2000)?;
            ensure!((1000..=5000).contains(&b), "buy-in must be 1000–5000 chips");
            let p = arg(&a, 2, policy::DEFAULT_PROFILE)?;
            ensure!(
                matches!(p, 20..=22 | policy::STICKY_LAB_PROFILE | v2::V2_PROFILE),
                "choose profile 20, 21, 22, 28, or 40"
            );
            live::run(b, p).await
        }
        Some("status") if a.len() == 1 => live::status().await,
        Some("performance") if a.len() <= 3 => {
            let n = arg(&a, 1, 1000usize)?;
            ensure!((1..=20000).contains(&n), "request 1–20000 hands");
            live::performance(n, a.get(2)).await
        }
        Some("arena") if a.len() <= 7 => {
            let b = arg(&a, 1, 1000usize)?;
            let n = arg(&a, 2, 6usize)?;
            let seed = arg(&a, 3, 1729u64)?;
            let samples = arg(&a, 4, live::SAMPLES)?;
            let profile = arg(&a, 5, policy::DEFAULT_PROFILE)?;
            let field = a.get(6).map(String::as_str).unwrap_or("mixed");
            ensure!(
                profile <= 20 && matches!(field, "scripted" | "mixed" | "baseline"),
                "profile 0–20, field scripted|mixed|baseline"
            );
            ensure!(
                (2..=1000000).contains(&b) && (2..=6).contains(&n) && (1..=8192).contains(&samples),
                "blocks 2–1000000, seats 2–6, samples 1–8192"
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&arena::bench(b, n, seed, samples, profile, field))?
            );
            Ok(())
        }
        Some("replay" | "shadow") if (2..=3).contains(&a.len()) => {
            let p = arg(&a, 2, policy::DEFAULT_PROFILE)?;
            ensure!(
                matches!(p, 20..=22 | policy::STICKY_LAB_PROFILE | v2::V2_PROFILE),
                "choose profile 20, 21, 22, 28, or 40"
            );
            shadow::run(Path::new(&a[1]), a[0] == "shadow", p)
        }
        Some("arena-v2") if a.len() <= 5 => {
            let sessions = arg(&a, 1, 120usize)?;
            let length = arg(&a, 2, 200usize)?;
            let seed = arg(&a, 3, 5_100_000_000u64)?;
            let samples = arg(&a, 4, 256usize)?;
            ensure!(
                (2..=2000).contains(&sessions)
                    && (10..=999).contains(&length)
                    && (32..=2048).contains(&samples),
                "invalid arena-v2 arguments"
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&arena::bench_v2(sessions, length, seed, samples))?
            );
            Ok(())
        }
        Some("debug-v2") if a.len() <= 4 => {
            let fi = arg(&a, 1, 0usize)?;
            let seed = arg(&a, 2, 5_100_000_000u64)?;
            let hands = arg(&a, 3, 600usize)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&arena::debug_hands(fi, seed, hands))?
            );
            Ok(())
        }
        Some("tune-v2") if a.len() <= 5 => {
            let iters = arg(&a, 1, 40usize)?;
            let sessions = arg(&a, 2, 72usize)?;
            let length = arg(&a, 3, 200usize)?;
            let seed = arg(&a, 4, 6_100_000_000u64)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&arena::tune_v2(iters, sessions, length, seed))?
            );
            Ok(())
        }
        Some("arena-guard") if a.len() <= 4 => {
            let n = arg(&a, 1, 300usize)?;
            let length = arg(&a, 2, 240usize)?;
            let seed = arg(&a, 3, 1_610_000_000u64)?;
            ensure!(
                (2..=5000).contains(&n)
                    && (1..=999).contains(&length)
                    && seed < u64::MAX - 40_000_000,
                "sessions 2–5000, hands 1–999, seed < u64::MAX - 40000000"
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&arena::guard_bench(n, length, seed))?
            );
            Ok(())
        }
        Some("arena-lab") if a.len() == 5 => {
            let n = arg(&a, 1, 300usize)?;
            let length = arg(&a, 2, 240usize)?;
            let seed = arg(&a, 3, 1_810_000_000u64)?;
            let candidate = arg(&a, 4, 0u8)?;
            ensure!(
                (2..=5000).contains(&n)
                    && (1..=999).contains(&length)
                    && seed < u64::MAX - 60_000_000
                    && matches!(candidate, 0 | 23..=28),
                "sessions 2–5000, hands 1–999, seed < u64::MAX - 60000000, candidate 0 (development) or 23–28"
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&arena::lab_bench(n, length, seed, candidate))?
            );
            Ok(())
        }
        Some("arena-fit-guard") if a.len() == 4 => {
            let n = arg(&a, 1, 600usize)?;
            let length = arg(&a, 2, 240usize)?;
            let seed = arg(&a, 3, 2_070_000_000u64)?;
            ensure!(
                (2..=5000).contains(&n)
                    && (1..=999).contains(&length)
                    && seed < u64::MAX - 60_000_000
            );
            println!(
                "{}",
                serde_json::to_string(&arena::guard_fit(n, length, seed))?
            );
            Ok(())
        }
        Some("arena-strategy") if a.len() == 5 => {
            let n = arg(&a, 1, 300usize)?;
            let length = arg(&a, 2, 240usize)?;
            let seed = arg(&a, 3, 2_450_000_000u64)?;
            let profile = arg(&a, 4, 0u8)?;
            ensure!(
                (2..=5000).contains(&n)
                    && (1..=999).contains(&length)
                    && seed < u64::MAX - 60_000_000
                    && matches!(profile, 0 | 29..=31),
                "invalid sessions, length, seed or candidate"
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&arena::strategy_bench(n, length, seed, profile))?
            );
            Ok(())
        }
        Some("arena-price") if a.len() == 5 => {
            let n = arg(&a, 1, 300usize)?;
            let length = arg(&a, 2, 240usize)?;
            let seed = arg(&a, 3, 2_650_000_000u64)?;
            let profile = arg(&a, 4, 0u8)?;
            ensure!(
                (2..=5000).contains(&n)
                    && (1..=999).contains(&length)
                    && seed < u64::MAX - 60_000_000
                    && matches!(profile, 0 | 32..=34),
                "invalid sessions, length, seed or candidate"
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&arena::price_bench(n, length, seed, profile))?
            );
            Ok(())
        }
        Some("policy-stdio") if a.len() == 1 => shadow::stdio(),
        Some("train-robust") if a.len() <= 3 => {
            let n = arg(&a, 1, 600000usize)?;
            let seed = arg(&a, 2, 38811001u64)?;
            ensure!((1000..=10000000).contains(&n), "1000–10000000 seed deals");
            println!(
                "{}",
                serde_json::to_string_pretty(&arena::robust_fit(n, seed))?
            );
            Ok(())
        }
        Some("train-preflop") if a.len() <= 3 => {
            let n = arg(&a, 1, 150000usize)?;
            let seed = arg(&a, 2, 452100701u64)?;
            ensure!((1000..=10000000).contains(&n), "1000–10000000 seed deals");
            println!("{}", serde_json::to_string_pretty(&arena::train(n, seed))?);
            Ok(())
        }
        None | Some("--help" | "-h") => {
            println!(
                "triplo-turbo live [buy-in] [profile 20|21|22|28]\ntriplo-turbo status\ntriplo-turbo performance [recent-account-hands] [hand-ids-file]\ntriplo-turbo arena [deal-blocks] [seats] [seed] [samples] [profile 0..20] [scripted|mixed|baseline]\ntriplo-turbo arena-guard [sessions] [hands-per-session] [seed]\ntriplo-turbo replay <wire.jsonl> [profile 20|21|22|28]\ntriplo-turbo shadow <growing-wire.jsonl> [profile 20|21|22|28]\ntriplo-turbo policy-stdio\ntriplo-turbo train-preflop [seed-deals] [master-seed]\ntriplo-turbo train-robust [seed-deals] [master-seed]"
            );
            Ok(())
        }
        _ => bail!("unknown command or arguments; see --help"),
    }
}
