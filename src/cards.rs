use rand::Rng;

/// Four contiguous 13-bit suit masks, deuce first. Construction is checked at ingress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Card(u8);
impl Card {
    pub fn parse(s: &str) -> Option<Self> {
        let &[r, suit] = s.as_bytes() else {
            return None;
        };
        let rank = b"23456789TJQKA".iter().position(|&x| x == r)?;
        let suit = b"cdhs".iter().position(|&x| x == suit)?;
        Some(Self((suit * 13 + rank) as u8))
    }
    pub fn rank(self) -> u8 {
        self.0 % 13 + 2
    }
    pub fn suit(self) -> u8 {
        self.0 / 13
    }
    pub fn bit(self) -> u64 {
        1 << self.0
    }
    pub fn text(self) -> String {
        format!(
            "{}{}",
            b"23456789TJQKA"[(self.0 % 13) as usize] as char,
            b"cdhs"[self.suit() as usize] as char
        )
    }
}

pub fn deck() -> [Card; 52] {
    std::array::from_fn(|i| Card(i as u8))
}
pub fn mask(cards: &[Card]) -> u64 {
    cards.iter().fold(0, |m, c| m | c.bit())
}
pub fn draw<R: Rng>(pool: &mut [Card; 52], n: &mut usize, rng: &mut R) -> Card {
    let i = rng.gen_range(0..*n);
    *n -= 1;
    pool.swap(i, *n);
    pool[*n]
}

fn high(m: u16) -> u32 {
    15 - m.leading_zeros()
}
fn top(mut m: u16, n: usize) -> u32 {
    for _ in n..m.count_ones() as usize {
        m &= m - 1;
    }
    m as u32
}
fn straight(m: u16) -> Option<u32> {
    let x = m & (m << 1) & (m << 2) & (m << 3) & (m << 4);
    if x != 0 {
        Some(high(x))
    } else if m & 0x100f == 0x100f {
        Some(3)
    } else {
        None
    }
}
/// Best five out of 5–7 distinct cards. Numeric ordering is showdown ordering.
pub fn evaluate(bits: u64) -> u32 {
    let [a, b, c, d] = std::array::from_fn(|i| ((bits >> (13 * i)) & 8191) as u16);
    let all = a | b | c | d;
    let flush = [a, b, c, d].into_iter().find(|s| s.count_ones() >= 5);
    if let Some(s) = flush.and_then(straight) {
        return 8 << 24 | s;
    }
    let quads = a & b & c & d;
    if quads != 0 {
        return 7 << 24 | high(quads) << 13 | top(all & !quads, 1);
    }
    let pairs = (a & b) | (c & d) | ((a | b) & (c | d));
    let trips = (a & b & (c | d)) | (c & d & (a | b));
    if trips != 0 {
        let t = high(trips);
        let rest = pairs & !(1 << t);
        if rest != 0 {
            return 6 << 24 | t << 13 | high(rest);
        }
    }
    if let Some(s) = flush {
        return 5 << 24 | top(s, 5);
    }
    if let Some(s) = straight(all) {
        return 4 << 24 | s;
    }
    if trips != 0 {
        return 3 << 24 | high(trips) << 13 | top(all & !trips, 2);
    }
    if pairs.count_ones() >= 2 {
        let p = top(pairs, 2);
        return 2 << 24
            | (high(p as u16) << 17)
            | (p.trailing_zeros() << 13)
            | top(all & !(p as u16), 1);
    }
    if pairs != 0 {
        return 1 << 24 | high(pairs) << 13 | top(all & !pairs, 3);
    }
    top(all, 5)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ev(s: &str) -> u32 {
        evaluate(mask(
            &s.split_whitespace()
                .map(|x| Card::parse(x).unwrap())
                .collect::<Vec<_>>(),
        ))
    }
    #[test]
    fn tricky_hands() {
        assert!(ev("Ac Ad Ah Ks Kh Kd 2s") > ev("Kc Kd Kh As Ah 3s 2s"));
        assert!(ev("2d 3c 4d 5s 6h") > ev("Ac 2d 3c 4d 5s"));
        assert_eq!(ev("2d 3h As Ks Qs Js Ts"), ev("Ad Ah As Ks Qs Js Ts"));
        assert!(ev("Ah Ad Kh Kd Qd Qs 2c") > ev("As Ac Kc Ks Jc Js 2h"));
    }
    #[test]
    fn exhaustive_five_card_census() {
        let mut counts = [0u32; 9];
        for a in 0..52 {
            for b in 0..a {
                for c in 0..b {
                    for d in 0..c {
                        for e in 0..d {
                            counts[(evaluate((1 << a) | (1 << b) | (1 << c) | (1 << d) | (1 << e))
                                >> 24) as usize] += 1;
                        }
                    }
                }
            }
        }
        assert_eq!(
            counts,
            [1302540, 1098240, 123552, 54912, 10200, 5108, 3744, 624, 40]
        );
    }
    #[test]
    fn seven_matches_best_subset() {
        use rand::{SeedableRng, seq::SliceRandom};
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(13);
        let mut d = deck();
        for _ in 0..20000 {
            d.partial_shuffle(&mut rng, 7);
            let seven = &d[45..];
            let m = mask(seven);
            let mut best = 0;
            for i in 0..7 {
                for j in 0..i {
                    best = best.max(evaluate(m ^ seven[i].bit() ^ seven[j].bit()));
                }
            }
            assert_eq!(evaluate(m), best);
        }
    }
}
