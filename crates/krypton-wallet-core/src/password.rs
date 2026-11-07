//! Password strength for wallet files.
//!
//! The wallet file can only be opened with its password (Argon2id + XChaCha20-Poly1305),
//! so the only attack on a stolen file is guessing the password offline. This estimate
//! is deliberately pessimistic: character pools, then penalties for the patterns that
//! guessing tools try first (common words, sequences, repeats, dates).

use serde::Serialize;

/// Bits a new wallet password needs (about 10^18 guesses).
pub const MIN_BITS: f64 = 60.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Strength {
    pub bits: f64,
    pub ok: bool,
    /// What to improve, if anything.
    pub hint: String,
}

const COMMON: &[&str] = &[
    "password", "passwort", "qwerty", "azerty", "123456", "12345", "111111", "iloveyou", "admin",
    "letmein", "welcome", "monkey", "dragon", "master", "sunshine", "princess", "football",
    "baseball", "shadow", "superman", "michael", "secret", "wallet", "bitcoin", "crypto",
    "genoryn", "krypton", "login", "abc123", "trustno1", "freedom", "whatever", "qazwsx",
];

fn has_run(s: &[u8], len: usize) -> bool {
    s.windows(len).any(|w| {
        let up = w.windows(2).all(|p| p[1] == p[0].wrapping_add(1));
        let down = w.windows(2).all(|p| p[0] == p[1].wrapping_add(1));
        up || down
    })
}

pub fn strength(pw: &str) -> Strength {
    let chars: Vec<char> = pw.chars().collect();
    let n = chars.len();
    if n == 0 {
        return Strength {
            bits: 0.0,
            ok: false,
            hint: "Choose a password.".into(),
        };
    }
    let mut pool = 0f64;
    if chars.iter().any(|c| c.is_ascii_lowercase()) {
        pool += 26.0;
    }
    if chars.iter().any(|c| c.is_ascii_uppercase()) {
        pool += 26.0;
    }
    if chars.iter().any(|c| c.is_ascii_digit()) {
        pool += 10.0;
    }
    if chars.iter().any(|c| c.is_ascii_punctuation()) {
        pool += 33.0;
    }
    if chars.contains(&' ') {
        pool += 1.0;
    }
    if chars.iter().any(|c| !c.is_ascii()) {
        pool += 60.0;
    }
    let mut bits = n as f64 * pool.max(1.0).log2();
    let lower = pw.to_lowercase();
    let mut hints = Vec::new();
    // Patterns guessing tools try first.
    for w in COMMON {
        if lower.contains(w) {
            bits -= 4.0 * w.len() as f64;
            hints.push("avoid common words like \"password\" or \"bitcoin\"");
            break;
        }
    }
    if has_run(lower.as_bytes(), 4)
        || ["qwer", "asdf", "zxcv", "1qaz"]
            .iter()
            .any(|k| lower.contains(k))
    {
        bits -= 12.0;
        hints.push("avoid sequences like \"abcd\", \"1234\" or keyboard rows");
    }
    let mut uniq = chars.clone();
    uniq.sort_unstable();
    uniq.dedup();
    if uniq.len() * 2 < n || uniq.len() < 5 {
        bits *= 0.5;
        hints.push("avoid repeating the same characters");
    }
    if chars.iter().all(|c| c.is_ascii_digit()) {
        bits *= 0.6;
    }
    if (1950..=2099).any(|y| lower.contains(&y.to_string())) {
        bits -= 6.0;
    }
    // A passphrase of several separate words is strong even in lowercase.
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .collect();
    if words.len() >= 4 {
        let mut distinct = words.clone();
        distinct.sort_unstable();
        distinct.dedup();
        bits = bits.max(distinct.len() as f64 * 11.0);
    }
    let bits = bits.max(0.0);
    let ok = bits >= MIN_BITS && n >= 12;
    let hint = if ok {
        String::new()
    } else if n < 12 {
        "Use at least 12 characters, or four or more unrelated words.".into()
    } else if let Some(h) = hints.first() {
        format!("Too easy to guess: {h}.")
    } else {
        "Too easy to guess: make it longer, or mix in digits and symbols.".into()
    };
    Strength {
        bits: (bits * 10.0).round() / 10.0,
        ok,
        hint,
    }
}

#[cfg(test)]
mod tests {
    use super::strength;

    #[test]
    fn weak_and_strong() {
        for weak in [
            "",
            "short1!",
            "Password123!",
            "aaaaaaaaaaaaaaaa",
            "123456789012",
            "qwertyuiop12",
            "bitcoin2026!!",
            "abcdefghijkl",
        ] {
            assert!(
                !strength(weak).ok,
                "{weak} should be weak: {:?}",
                strength(weak)
            );
        }
        for strong in [
            "correct horse battery staple",
            "T7#vq9!Lm2@xR4",
            "my cat eats green rockets daily",
            "Zebra-Lantern-Fjord-77",
        ] {
            assert!(
                strength(strong).ok,
                "{strong} should be strong: {:?}",
                strength(strong)
            );
        }
        assert!(strength("Password123!").hint.contains("common"));
    }
}
