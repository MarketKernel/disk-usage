//! Human-readable formatting of sizes and counts.

/// macOS Finder uses decimal units, Windows Explorer and most Linux tools use binary ones.
#[cfg(target_os = "macos")]
const BASE: f64 = 1000.0;
#[cfg(not(target_os = "macos"))]
const BASE: f64 = 1024.0;

const UNITS: [&str; 7] = ["B", "KB", "MB", "GB", "TB", "PB", "EB"];

pub fn bytes(n: u64) -> String {
    bytes_with_base(n, BASE)
}

fn bytes_with_base(n: u64, base: f64) -> String {
    let mut value = n as f64;
    let mut unit = 0;
    while value >= base && unit < UNITS.len() - 1 {
        value /= base;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `1234567` -> `1 234 567` (thin-space grouping reads well in any locale).
pub fn count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 * 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(ch);
    }
    out
}

pub fn percent(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "—".to_owned();
    }
    let p = part as f64 * 100.0 / whole as f64;
    if p >= 10.0 {
        format!("{p:.0}%")
    } else if p >= 0.1 {
        format!("{p:.1}%")
    } else {
        "<0.1%".to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes_with_base(0, 1000.0), "0 B");
        assert_eq!(bytes_with_base(999, 1000.0), "999 B");
        assert_eq!(bytes_with_base(1500, 1000.0), "1.5 KB");
        assert_eq!(bytes_with_base(474_300_000_000, 1000.0), "474 GB");
        assert_eq!(bytes_with_base(47_430_000_000, 1000.0), "47.4 GB");
        assert_eq!(bytes_with_base(1024 * 1024, 1024.0), "1.0 MB");
    }

    #[test]
    fn formats_counts() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1\u{2009}000");
        assert_eq!(count(1234567), "1\u{2009}234\u{2009}567");
    }

    #[test]
    fn formats_percent() {
        assert_eq!(percent(1, 0), "—");
        assert_eq!(percent(50, 100), "50%");
        assert_eq!(percent(5, 100), "5.0%");
        assert_eq!(percent(1, 100_000), "<0.1%");
    }
}
