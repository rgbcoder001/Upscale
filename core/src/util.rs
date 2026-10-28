//! Number parsing and formatting that match the reference script (.NET invariant culture).

/// `ConvertFrom-Rational`: "24000/1001" → 23.976…, "0/0" → 0 (ffprobe's "unknown"), "25" → 25.
/// `None` where the script's `[double]::Parse` would throw.
pub fn parse_rational(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Some((num, den)) = s.split_once('/') {
        let (num, den) = (num.trim(), den.trim());
        let is_num = |t: &str, signed: bool| {
            let t = if signed {
                t.strip_prefix('-').unwrap_or(t)
            } else {
                t
            };
            !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        };
        if is_num(num, true) && is_num(den, false) {
            let den: f64 = den.parse().ok()?;
            if den == 0.0 {
                return Some(0.0);
            }
            return Some(num.parse::<f64>().ok()? / den);
        }
    }
    s.parse().ok()
}

/// `[int]` / `[Math]::Round` in PowerShell: round half to even.
pub fn round_even(v: f64) -> i64 {
    v.round_ties_even() as i64
}

/// Rounds like .NET formatting (15 significant digits, then half away from zero) and returns
/// (negative, integer digits, exactly `decimals` fraction digits).
fn round_digits(v: f64, decimals: usize) -> (bool, String, String) {
    if v == 0.0 || !v.is_finite() {
        return (false, "0".into(), "0".repeat(decimals));
    }
    let s = format!("{:.14e}", v.abs());
    let (mant, exp) = s.split_once('e').expect("exponent form");
    let exp: i32 = exp.parse().expect("exponent");
    let mut digits: Vec<u8> = mant
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    let mut int_len = exp + 1;
    if int_len <= 0 {
        let mut padded = vec![0u8; (1 - int_len) as usize];
        padded.extend(digits);
        digits = padded;
        int_len = 1;
    }
    let keep = int_len as usize + decimals;
    if digits.len() < keep {
        digits.resize(keep, 0);
    }
    let round_up = digits.len() > keep && digits[keep] >= 5;
    digits.truncate(keep);
    if round_up {
        let mut i = keep;
        loop {
            if i == 0 {
                digits.insert(0, 1);
                break;
            }
            i -= 1;
            if digits[i] == 9 {
                digits[i] = 0;
            } else {
                digits[i] += 1;
                break;
            }
        }
    }
    let split = digits.len() - decimals;
    let to_str = |d: &[u8]| d.iter().map(|d| (d + b'0') as char).collect::<String>();
    let int = to_str(&digits[..split]);
    let int = int.trim_start_matches('0');
    let int = if int.is_empty() { "0" } else { int }.to_string();
    let frac = to_str(&digits[split..]);
    let negative = v < 0.0 && (int != "0" || frac.bytes().any(|b| b != b'0'));
    (negative, int, frac)
}

/// .NET custom format `'0.###…'` with `max_decimals` `#`s: trailing zeros dropped.
pub fn fmt_inv(v: f64, max_decimals: usize) -> String {
    let (neg, int, frac) = round_digits(v, max_decimals);
    let frac = frac.trim_end_matches('0');
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(&int);
    if !frac.is_empty() {
        out.push('.');
        out.push_str(frac);
    }
    out
}

/// .NET standard format `N<decimals>` (en-US grouping): `1,234.500`.
pub fn fmt_n(v: f64, decimals: usize) -> String {
    let (neg, int, frac) = round_digits(v, decimals);
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    out.push_str(&grouped);
    if decimals > 0 {
        out.push('.');
        out.push_str(&frac);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rationals() {
        assert!((parse_rational("24000/1001").unwrap() - 23.976_023_976).abs() < 1e-9);
        assert_eq!(parse_rational("0/0"), Some(0.0));
        assert_eq!(parse_rational(" 25 "), Some(25.0));
        assert_eq!(parse_rational("34000/50000"), Some(0.68));
        assert_eq!(parse_rational("n/a"), None);
    }

    #[test]
    fn invariant_custom_format() {
        assert_eq!(fmt_inv(0.0, 4), "0");
        assert_eq!(fmt_inv(0.5, 4), "0.5");
        assert_eq!(fmt_inv(1.0 / 3.0, 4), "0.3333");
        assert_eq!(fmt_inv(0.03125, 4), "0.0313"); // half away from zero, unlike Rust's {:.4}
        assert_eq!(fmt_inv(1528.027, 3), "1528.027");
        assert_eq!(fmt_inv(0.99996, 4), "1");
        assert_eq!(fmt_inv(-0.00001, 4), "0");
        assert_eq!(fmt_inv(1000.0, 4), "1000");
    }

    #[test]
    fn standard_n_format() {
        assert_eq!(fmt_n(24000.0 / 1001.0, 3), "23.976");
        assert_eq!(fmt_n(25.0, 3), "25.000");
        assert_eq!(fmt_n(269.6, 0), "270");
        assert_eq!(fmt_n(1234.5, 0), "1,235");
        assert_eq!(fmt_n(0.51, 2), "0.51");
    }

    #[test]
    fn banker_rounding() {
        assert_eq!(round_even(2.5), 2);
        assert_eq!(round_even(3.5), 4);
        assert_eq!(round_even(-0.4), 0);
    }
}
