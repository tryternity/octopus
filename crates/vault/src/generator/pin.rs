//! 纯数字 PIN。

use anyhow::{ensure, Result};
use rand::rngs::OsRng;
use rand::Rng;

use super::PinConfig;

pub fn generate(cfg: &PinConfig) -> Result<String> {
    ensure!(cfg.length >= 1, "PIN 长度必须 ≥ 1（当前 {}）", cfg.length);
    ensure!(cfg.length <= 32, "PIN 长度必须 ≤ 32（当前 {}）", cfg.length);
    let mut rng = OsRng;
    let s: String = (0..cfg.length)
        .map(|_| char::from_digit(rng.gen_range(0..10), 10).unwrap())
        .collect();
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pin_length_default() {
        let cfg = PinConfig::default();
        for _ in 0..100 {
            let s = generate(&cfg).unwrap();
            assert_eq!(s.len(), 6);
            assert!(s.chars().all(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn test_custom_length() {
        let cfg = PinConfig { length: 4 };
        let s = generate(&cfg).unwrap();
        assert_eq!(s.len(), 4);
    }

    #[test]
    fn test_too_short_errors() {
        let cfg = PinConfig { length: 0 };
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn test_too_long_errors() {
        let cfg = PinConfig { length: 33 };
        assert!(generate(&cfg).is_err());
    }

    #[test]
    fn test_length_bounds_ok() {
        let cfg_min = PinConfig { length: 1 };
        assert!(generate(&cfg_min).is_ok());
        let cfg_max = PinConfig { length: 32 };
        assert!(generate(&cfg_max).is_ok());
    }
}
