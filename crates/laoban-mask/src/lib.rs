//! Sensitive data masking for laoban.
//!
//! One rule set is applied in two places:
//!
//! * before any text is sent to the AI provider ([`Destination::Ai`]);
//! * before any text is written to the audit log ([`Destination::Audit`]).
//!
//! The only difference between the two destinations is login accounts:
//! they are masked in content sent to the AI, but kept in audit logs,
//! where the actor is recorded as a Rancher principal ID for traceability.
//!
//! IP addresses are intentionally never masked: the AI needs them to plan
//! network changes.
//!
//! Masking is biased towards over-masking. A long digit run that happens to
//! pass the Luhn check is masked as a payment card even if it is something
//! else; losing a timestamp is cheaper than leaking a card number.
//!
//! [`Finding`] never contains the original value, so findings are safe to log.

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod detect;

pub use detect::{luhn_valid, tw_id_checksum_valid};

/// Where the masked text is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destination {
    /// Content sent to the AI provider. Login accounts are masked.
    Ai,
    /// Content written to the audit log. Login accounts are kept.
    Audit,
}

/// What kind of sensitive data was found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "name")]
pub enum Kind {
    /// Taiwan national ID (身分證字號), e.g. `A1xxxxxxxx`.
    TwNationalId,
    /// Taiwan resident certificate number (居留證號), old or new format.
    TwResidentId,
    /// Payment card number that passes the Luhn check.
    PaymentCard,
    /// Bank or financial account number, detected by surrounding keywords.
    FinancialAccount,
    /// A login account name supplied by the caller.
    LoginAccount,
    /// A credential: an API key or access token with a known prefix, a
    /// private key block, or the value after `password=`, `token:` and the
    /// like.
    Credential,
    /// An administrator-defined rule.
    Custom(String),
}

impl Kind {
    /// Stable token used in the replacement text. ASCII on purpose so it
    /// survives any downstream encoding and is understood by the AI.
    pub fn token(&self) -> String {
        match self {
            Kind::TwNationalId => "[MASKED:TW_ID]".to_string(),
            Kind::TwResidentId => "[MASKED:TW_RESIDENT_ID]".to_string(),
            Kind::PaymentCard => "[MASKED:CARD]".to_string(),
            Kind::FinancialAccount => "[MASKED:ACCOUNT]".to_string(),
            Kind::LoginAccount => "[MASKED:LOGIN]".to_string(),
            Kind::Credential => "[MASKED:SECRET]".to_string(),
            Kind::Custom(name) => format!("[MASKED:{}]", custom_token_name(name)),
        }
    }
}

fn custom_token_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "CUSTOM".to_string()
    } else {
        format!("CUSTOM_{cleaned}")
    }
}

/// One masked span. Offsets are byte offsets into the original text.
/// The original value is deliberately not stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    /// For ID numbers: whether the check digit is valid. Invalid ones are
    /// still masked, because a typo in an ID is still personal data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checksum_valid: Option<bool>,
}

/// Result of masking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Masked {
    pub text: String,
    pub findings: Vec<Finding>,
}

/// An administrator-defined masking rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomRule {
    pub name: String,
    /// Regular expression, `regex` crate syntax.
    pub pattern: String,
    /// Case-insensitive matching.
    #[serde(default)]
    pub case_insensitive: bool,
}

/// Masking configuration. Defaults match laoban's phase-1 decisions:
/// national IDs, financial accounts, credentials and login accounts (for AI)
/// are masked; IP addresses are not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MaskerConfig {
    pub mask_national_ids: bool,
    pub mask_financial_accounts: bool,
    pub mask_login_accounts_for_ai: bool,
    /// API keys, tokens, private keys and passwords (both destinations).
    pub mask_credentials: bool,
    /// Login account names known to Rancher. Matched case-insensitively.
    pub login_accounts: Vec<String>,
    pub custom_rules: Vec<CustomRule>,
}

impl Default for MaskerConfig {
    fn default() -> Self {
        Self {
            mask_national_ids: true,
            mask_financial_accounts: true,
            mask_login_accounts_for_ai: true,
            mask_credentials: true,
            login_accounts: Vec::new(),
            custom_rules: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum MaskError {
    #[error("custom rule `{name}` has an invalid pattern: {source}")]
    InvalidCustomRule {
        name: String,
        #[source]
        source: regex::Error,
    },
    #[error("custom rule `{name}` matches the empty string, which would mask nothing useful")]
    EmptyMatchingRule { name: String },
}

/// Compiled masker. Build once, reuse for every request.
#[derive(Debug)]
pub struct Masker {
    config: MaskerConfig,
    custom: Vec<(String, Regex)>,
    login_patterns: Vec<Regex>,
}

impl Masker {
    pub fn new(config: MaskerConfig) -> Result<Self, MaskError> {
        let mut custom = Vec::with_capacity(config.custom_rules.len());
        for rule in &config.custom_rules {
            let re = RegexBuilder::new(&rule.pattern)
                .case_insensitive(rule.case_insensitive)
                .build()
                .map_err(|source| MaskError::InvalidCustomRule {
                    name: rule.name.clone(),
                    source,
                })?;
            if re.is_match("") {
                return Err(MaskError::EmptyMatchingRule {
                    name: rule.name.clone(),
                });
            }
            custom.push((rule.name.clone(), re));
        }
        let mut login_patterns = Vec::new();
        for account in config.login_accounts.iter().map(|a| a.trim()) {
            if account.is_empty() {
                continue;
            }
            let re = RegexBuilder::new(&regex::escape(account))
                .case_insensitive(true)
                .build()
                .expect("an escaped literal is always a valid pattern");
            login_patterns.push(re);
        }
        Ok(Self {
            config,
            custom,
            login_patterns,
        })
    }

    pub fn config(&self) -> &MaskerConfig {
        &self.config
    }

    /// Mask `text` for the given destination.
    pub fn mask(&self, text: &str, destination: Destination) -> Masked {
        let mut findings = Vec::new();

        if self.config.mask_credentials {
            findings.extend(detect::credentials(text));
        }
        if self.config.mask_national_ids {
            findings.extend(detect::tw_ids(text));
        }
        if self.config.mask_financial_accounts {
            findings.extend(detect::payment_cards(text));
            findings.extend(detect::financial_accounts(text));
        }
        if destination == Destination::Ai && self.config.mask_login_accounts_for_ai {
            findings.extend(detect::login_accounts(text, &self.login_patterns));
        }
        for (name, re) in &self.custom {
            for m in re.find_iter(text) {
                if m.start() < m.end() {
                    findings.push(Finding {
                        kind: Kind::Custom(name.clone()),
                        start: m.start(),
                        end: m.end(),
                        checksum_valid: None,
                    });
                }
            }
        }

        let findings = resolve_overlaps(findings);
        let text = apply(text, &findings);
        Masked { text, findings }
    }
}

/// Keep non-overlapping findings. When two overlap, the one that starts
/// first wins; on a tie, the longer one wins. The covered range of a loser is
/// still masked because the winner's range is extended to cover it.
fn resolve_overlaps(mut findings: Vec<Finding>) -> Vec<Finding> {
    findings.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut out: Vec<Finding> = Vec::with_capacity(findings.len());
    for f in findings {
        match out.last_mut() {
            Some(last) if f.start < last.end => {
                if f.end > last.end {
                    last.end = f.end;
                }
            }
            _ => out.push(f),
        }
    }
    out
}

fn apply(text: &str, findings: &[Finding]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for f in findings {
        out.push_str(&text[cursor..f.start]);
        out.push_str(&f.kind.token());
        cursor = f.end;
    }
    out.push_str(&text[cursor..]);
    out
}
