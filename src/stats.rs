use serde_json::{Value, json};
/// Chronological net BB per hand; grants and buy-ins never enter this series.
pub fn performance(x: &[f64]) -> Value {
    if x.is_empty() {
        return json!({"hands":0});
    }
    let mean = x.iter().sum::<f64>() / x.len() as f64;
    let (mut balance, mut high, mut dd) = (0.0, 0.0_f64, 0.0_f64);
    for &v in x {
        balance += v;
        high = high.max(balance);
        dd = dd.max(high - balance);
    }
    // Cluster sandwich standard error for the ratio sum(chips in BB) / hands.
    // Keep the last partial cluster; return no interval for short samples.
    let b = x.chunks(100).count();
    let ci = (b >= 10).then(|| {
        let rss = x
            .chunks(100)
            .map(|c| c.iter().map(|v| v - mean).sum::<f64>().powi(2))
            .sum::<f64>();
        let d = 1.96 * 100.0 * (rss * b as f64 / (b - 1) as f64).sqrt() / x.len() as f64;
        [100.0 * mean - d, 100.0 * mean + d]
    });
    json!({"hands":x.len(),"net_bb":balance,"bb_per_100":100.0*mean,"approx_block_95pct_ci":ci,"block_size":100,"max_drawdown_bb":dd,"net_positive_hands":x.iter().filter(|&&v|v>0.0).count()})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drawdown_from_peak() {
        let r = performance(&[5.0, -4.0, 1.0, -6.0, 2.0]);
        assert_eq!(r["max_drawdown_bb"], 9.0);
        assert_eq!(r["bb_per_100"], -40.0);
        assert!(r["approx_block_95pct_ci"].is_null());
    }
}
