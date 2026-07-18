//! 纯数字 PIN。

use rand::rngs::OsRng;
use rand::Rng;

use super::PinConfig;

pub fn generate(cfg: &PinConfig) -> String {
    assert!(cfg.length >= 1, "PIN length 必须 >= 1");
    assert!(cfg.length <= 32, "PIN length 必须 <= 32");
    let mut rng = OsRng;
    (0..cfg.length)
        .map(|_| char::from_digit(rng.gen_range(0..10), 10).unwrap())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pin_length_default() {
        let cfg = PinConfig::default();
        for _ in 0..100 {
            let s = generate(&cfg);
            assert_eq!(s.len(), 6);
            assert!(s.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn test_custom_length() {
        let cfg = PinConfig { length: 4 };
        let s = generate(&cfg);
        assert_eq!(s.len(), 4);
    }
}
