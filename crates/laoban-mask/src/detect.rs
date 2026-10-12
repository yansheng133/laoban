//! Detectors. Each returns findings with byte offsets into the input.

use std::sync::LazyLock;

use regex::Regex;

use crate::{Finding, Kind};

/// Taiwan ID-like token: one letter, then a second character that is
/// `1`/`2` (national ID), `8`/`9` (new resident certificate) or `A`-`D`
/// (old resident certificate), then eight digits.
static TW_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z][1289A-Da-d]\d{8}").expect("valid regex"));

/// 13 to 19 digits, optionally separated by single spaces or dashes.
static CARD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d(?:[ -]?\d){12,18}").expect("valid regex"));

/// An account keyword followed by a digit run. Only the digit run is masked.
static ACCOUNT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:存款帳號|帳號|帳戶|戶號|account\s*(?:no\.?|number|#)?|acct\.?|a/c)\s*[:：#]?\s*(\d[\d -]{8,24}\d)",
    )
    .expect("valid regex")
});

fn is_ascii_alnum_at(text: &str, idx: usize, before: bool) -> bool {
    let c = if before {
        text[..idx].chars().next_back()
    } else {
        text[idx..].chars().next()
    };
    matches!(c, Some(c) if c.is_ascii_alphanumeric())
}

fn is_ascii_digit_at(text: &str, idx: usize, before: bool) -> bool {
    let c = if before {
        text[..idx].chars().next_back()
    } else {
        text[idx..].chars().next()
    };
    matches!(c, Some(c) if c.is_ascii_digit())
}

/// Letter codes used by the Taiwan ID check digit.
fn tw_letter_code(c: char) -> Option<u32> {
    let code = match c.to_ascii_uppercase() {
        'A' => 10,
        'B' => 11,
        'C' => 12,
        'D' => 13,
        'E' => 14,
        'F' => 15,
        'G' => 16,
        'H' => 17,
        'I' => 34,
        'J' => 18,
        'K' => 19,
        'L' => 20,
        'M' => 21,
        'N' => 22,
        'O' => 35,
        'P' => 23,
        'Q' => 24,
        'R' => 25,
        'S' => 26,
        'T' => 27,
        'U' => 28,
        'V' => 29,
        'W' => 32,
        'X' => 30,
        'Y' => 31,
        'Z' => 33,
        _ => return None,
    };
    Some(code)
}

/// Check digit validation for Taiwan national IDs and resident certificate
/// numbers (old format with a letter in position 2, new format with 8/9).
pub fn tw_id_checksum_valid(id: &str) -> bool {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() != 10 {
        return false;
    }
    let Some(first) = tw_letter_code(chars[0]) else {
        return false;
    };
    let second = if chars[1].is_ascii_digit() {
        chars[1].to_digit(10).unwrap_or(0)
    } else {
        match tw_letter_code(chars[1]) {
            Some(code) => code % 10,
            None => return false,
        }
    };
    let mut digits = Vec::with_capacity(8);
    for c in &chars[2..] {
        match c.to_digit(10) {
            Some(d) => digits.push(d),
            None => return false,
        }
    }
    let mut sum = (first / 10) + (first % 10) * 9 + second * 8;
    for (i, d) in digits[..7].iter().enumerate() {
        sum += d * (7 - i as u32);
    }
    sum += digits[7];
    sum % 10 == 0
}

pub(crate) fn tw_ids(text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for m in TW_ID.find_iter(text) {
        if is_ascii_alnum_at(text, m.start(), true) || is_ascii_alnum_at(text, m.end(), false) {
            continue;
        }
        let s = m.as_str();
        let second = s.as_bytes()[1];
        let kind = if second == b'1' || second == b'2' {
            Kind::TwNationalId
        } else {
            Kind::TwResidentId
        };
        out.push(Finding {
            kind,
            start: m.start(),
            end: m.end(),
            checksum_valid: Some(tw_id_checksum_valid(s)),
        });
    }
    out
}

/// Luhn check on a string of ASCII digits.
pub fn luhn_valid(digits: &str) -> bool {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let mut sum = 0u32;
    for (i, b) in digits.bytes().rev().enumerate() {
        let mut d = u32::from(b - b'0');
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum % 10 == 0
}

pub(crate) fn payment_cards(text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for m in CARD.find_iter(text) {
        if is_ascii_digit_at(text, m.start(), true) || is_ascii_digit_at(text, m.end(), false) {
            continue;
        }
        let digits: String = m.as_str().chars().filter(char::is_ascii_digit).collect();
        if (13..=19).contains(&digits.len()) && luhn_valid(&digits) {
            out.push(Finding {
                kind: Kind::PaymentCard,
                start: m.start(),
                end: m.end(),
                checksum_valid: None,
            });
        }
    }
    out
}

pub(crate) fn financial_accounts(text: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for caps in ACCOUNT.captures_iter(text) {
        let Some(g) = caps.get(1) else { continue };
        if is_ascii_digit_at(text, g.end(), false) {
            continue;
        }
        let digit_count = g.as_str().chars().filter(char::is_ascii_digit).count();
        if (10..=16).contains(&digit_count) {
            out.push(Finding {
                kind: Kind::FinancialAccount,
                start: g.start(),
                end: g.end(),
                checksum_valid: None,
            });
        }
    }
    out
}

/// Characters that may appear inside a login account name. A match is only
/// accepted when it is not glued to one of these on either side, so `sam`
/// does not match inside `samuel` or `sam.lin`.
fn is_account_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '@')
}

pub(crate) fn login_accounts(text: &str, patterns: &[Regex]) -> Vec<Finding> {
    let mut out = Vec::new();
    for re in patterns {
        for m in re.find_iter(text) {
            let before = text[..m.start()].chars().next_back();
            let after = text[m.end()..].chars().next();
            if before.is_some_and(is_account_char) || after.is_some_and(is_account_char) {
                continue;
            }
            out.push(Finding {
                kind: Kind::LoginAccount,
                start: m.start(),
                end: m.end(),
                checksum_valid: None,
            });
        }
    }
    out
}
