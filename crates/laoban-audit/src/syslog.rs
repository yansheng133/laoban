//! RFC 5424 formatting and RFC 5425 framing.

use std::fmt::Write as _;

use laoban_mask::{Destination, Masker};
use thiserror::Error;

use crate::event::AuditEvent;

/// RFC 5424 NILVALUE.
pub const NILVALUE: &str = "-";

/// Default SD-ID. 32473 is the Private Enterprise Number that RFC 5612
/// reserves for documentation examples. **Replace it in production** with
/// an SD-ID under a PEN actually assigned by IANA.
pub const DEFAULT_SD_ID: &str = "laoban@32473";

/// Field limits from RFC 5424 section 6 (ABNF).
pub const HOSTNAME_MAX: usize = 255;
pub const APP_NAME_MAX: usize = 48;
pub const PROCID_MAX: usize = 128;
pub const MSGID_MAX: usize = 32;
/// SD-NAME (used for SD-ID and PARAM-NAME) limit.
pub const SD_NAME_MAX: usize = 32;

/// Built-in SD-PARAM names. Event fields may not reuse them.
pub const RESERVED_PARAM_NAMES: &[&str] =
    &["actor", "action", "outcome", "project", "target", "masked"];

/// Syslog facility, RFC 5424 section 6.2.1 table 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Facility {
    Kern = 0,
    User = 1,
    Mail = 2,
    Daemon = 3,
    Auth = 4,
    Syslog = 5,
    Lpr = 6,
    News = 7,
    Uucp = 8,
    Cron = 9,
    AuthPriv = 10,
    Ftp = 11,
    Ntp = 12,
    LogAudit = 13,
    LogAlert = 14,
    Clock = 15,
    Local0 = 16,
    Local1 = 17,
    Local2 = 18,
    Local3 = 19,
    Local4 = 20,
    Local5 = 21,
    Local6 = 22,
    Local7 = 23,
}

impl Facility {
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// Syslog severity, RFC 5424 section 6.2.1 table 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Severity {
    Emergency = 0,
    Alert = 1,
    Critical = 2,
    Error = 3,
    Warning = 4,
    Notice = 5,
    Informational = 6,
    Debug = 7,
}

impl Severity {
    pub fn code(self) -> u8 {
        self as u8
    }
}

/// PRI value: facility * 8 + severity (RFC 5424 section 6.2.1).
pub fn pri(facility: Facility, severity: Severity) -> u8 {
    facility.code() * 8 + severity.code()
}

/// Header and SD-ID settings. Validated by [`AuditFormatter::new`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatterConfig {
    /// HOSTNAME. Empty renders as NILVALUE.
    pub hostname: String,
    /// APP-NAME. Empty renders as NILVALUE.
    pub app_name: String,
    /// PROCID. Empty renders as NILVALUE.
    pub procid: String,
    /// Default `Local0`: local0 is what customers' syslog/SIEM rules most
    /// often route application audit streams by. `LogAudit` (13) is the
    /// semantically closest standard facility if the customer prefers it.
    pub facility: Facility,
    /// SD-ID of the single structured data element. Must be a private
    /// SD-ID `name@<PEN>`. See [`DEFAULT_SD_ID`].
    pub sd_id: String,
}

impl Default for FormatterConfig {
    fn default() -> Self {
        Self {
            hostname: String::new(),
            app_name: "laoban".to_string(),
            procid: String::new(),
            facility: Facility::Local0,
            sd_id: DEFAULT_SD_ID.to_string(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("{field} is {len} characters, the limit is {max}")]
    TooLong {
        field: &'static str,
        len: usize,
        max: usize,
    },
    #[error("{field} may only contain printable US-ASCII without spaces (%d33-126)")]
    NotPrintUsAscii { field: &'static str },
    #[error("SD-ID `{0}` must have the form name@<private enterprise number>, at most 32 characters, without `=`, `]`, `\"` or spaces")]
    InvalidSdId(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormatError {
    #[error("field name `{0}` is not a valid SD-PARAM name (1-32 printable US-ASCII, no `=`, `]`, `\"`, `@` or spaces)")]
    InvalidFieldName(String),
    #[error("field name `{0}` is reserved for a built-in parameter")]
    ReservedFieldName(String),
    #[error("field name `{0}` is used more than once")]
    DuplicateFieldName(String),
}

/// A formatted RFC 5424 message (`SYSLOG-MSG`, without framing or trailing
/// newline). Can only be produced by [`AuditFormatter::format`], so every
/// record has gone through masking. Guaranteed to contain no CR or LF.
#[derive(Clone, PartialEq, Eq)]
pub struct SyslogRecord(String);

impl SyslogRecord {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Length in octets (bytes), not characters.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for SyslogRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SyslogRecord").field(&self.0).finish()
    }
}

impl std::fmt::Display for SyslogRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// RFC 5425 section 4.3 octet-counting frame: `MSG-LEN SP SYSLOG-MSG`,
/// where MSG-LEN is the length of SYSLOG-MSG in octets.
pub fn frame_octet_counting(record: &SyslogRecord) -> Vec<u8> {
    let msg = record.as_bytes();
    let prefix = format!("{} ", msg.len());
    let mut out = Vec::with_capacity(prefix.len() + msg.len());
    out.extend_from_slice(prefix.as_bytes());
    out.extend_from_slice(msg);
    out
}

/// Formats [`AuditEvent`]s as RFC 5424 messages. Owns the [`Masker`], so
/// there is no way to render an event without masking it.
#[derive(Debug)]
pub struct AuditFormatter {
    config: FormatterConfig,
    masker: Masker,
}

impl AuditFormatter {
    pub fn new(config: FormatterConfig, masker: Masker) -> Result<Self, ConfigError> {
        validate_header("HOSTNAME", &config.hostname, HOSTNAME_MAX)?;
        validate_header("APP-NAME", &config.app_name, APP_NAME_MAX)?;
        validate_header("PROCID", &config.procid, PROCID_MAX)?;
        validate_sd_id(&config.sd_id)?;
        Ok(Self { config, masker })
    }

    pub fn config(&self) -> &FormatterConfig {
        &self.config
    }

    /// Mask and format one event. The event is consumed so the unmasked
    /// text does not outlive formatting in this crate.
    ///
    /// Layout (RFC 5424 section 6):
    /// `<PRI>1 TIMESTAMP HOSTNAME APP-NAME PROCID MSGID [SD-ID params] MSG`
    pub fn format(&self, event: AuditEvent) -> Result<SyslogRecord, FormatError> {
        let mut seen: Vec<&str> = Vec::with_capacity(event.fields.len());
        for (key, _) in &event.fields {
            if !is_param_name(key) {
                return Err(FormatError::InvalidFieldName(key.clone()));
            }
            if RESERVED_PARAM_NAMES.contains(&key.as_str()) {
                return Err(FormatError::ReservedFieldName(key.clone()));
            }
            if seen.contains(&key.as_str()) {
                return Err(FormatError::DuplicateFieldName(key.clone()));
            }
            seen.push(key);
        }

        let mut masked_count = 0usize;
        let mut mask = |text: &str| -> String {
            let m = self.masker.mask(text, Destination::Audit);
            masked_count += m.findings.len();
            escape_controls(&m.text)
        };

        let project = mask(&event.project);
        let target = mask(&event.target);
        let detail = mask(&event.detail);
        let fields: Vec<(&str, String)> = event
            .fields
            .iter()
            .map(|(k, v)| (k.as_str(), mask(v)))
            .collect();
        // The actor is a principal ID and is kept, only made line-safe.
        let actor = escape_controls(&event.actor);

        let mut out = String::with_capacity(256 + detail.len());
        let _ = write!(
            out,
            "<{}>1 {} {} {} {} {} [{}",
            pri(self.config.facility, event.outcome.severity()),
            event.timestamp,
            nil_if_empty(&self.config.hostname),
            nil_if_empty(&self.config.app_name),
            nil_if_empty(&self.config.procid),
            event.action.as_str(),
            self.config.sd_id,
        );
        push_param(&mut out, "actor", &actor);
        push_param(&mut out, "action", event.action.as_str());
        push_param(&mut out, "outcome", event.outcome.as_str());
        if !project.is_empty() {
            push_param(&mut out, "project", &project);
        }
        if !target.is_empty() {
            push_param(&mut out, "target", &target);
        }
        for (k, v) in &fields {
            push_param(&mut out, k, v);
        }
        push_param(&mut out, "masked", &masked_count.to_string());
        out.push(']');

        if !detail.is_empty() {
            out.push(' ');
            // RFC 5424 section 6.4: MSG encoded in UTF-8 MUST start with
            // the BOM. Pure ASCII is emitted as MSG-ANY without a BOM.
            if !detail.is_ascii() {
                out.push('\u{FEFF}');
            }
            out.push_str(&detail);
        }
        Ok(SyslogRecord(out))
    }
}

fn nil_if_empty(s: &str) -> &str {
    if s.is_empty() {
        NILVALUE
    } else {
        s
    }
}

fn is_print_us_ascii(s: &str) -> bool {
    s.bytes().all(|b| (33..=126).contains(&b))
}

fn validate_header(field: &'static str, value: &str, max: usize) -> Result<(), ConfigError> {
    if value.len() > max {
        return Err(ConfigError::TooLong {
            field,
            len: value.len(),
            max,
        });
    }
    if !is_print_us_ascii(value) {
        return Err(ConfigError::NotPrintUsAscii { field });
    }
    Ok(())
}

/// SD-NAME: 1*32 PRINTUSASCII except `=`, SP, `]`, `"`.
fn is_sd_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= SD_NAME_MAX
        && s.bytes()
            .all(|b| (33..=126).contains(&b) && !matches!(b, b'=' | b']' | b'"'))
}

/// PARAM-NAME is an SD-NAME; we additionally reject `@`, which RFC 5424
/// section 6.3.3 reserves for private parameter names under IANA SD-IDs.
fn is_param_name(s: &str) -> bool {
    is_sd_name(s) && !s.contains('@')
}

/// Private SD-ID (RFC 5424 section 7.2.2): `name@<PEN>`, exactly one `@`,
/// the right side numeric. Dotted sub-identifiers are not accepted.
fn validate_sd_id(s: &str) -> Result<(), ConfigError> {
    let ok = is_sd_name(s)
        && match s.split_once('@') {
            Some((name, pen)) => {
                !name.is_empty()
                    && !pen.is_empty()
                    && !pen.contains('@')
                    && pen.bytes().all(|b| b.is_ascii_digit())
            }
            None => false,
        };
    if ok {
        Ok(())
    } else {
        Err(ConfigError::InvalidSdId(s.to_string()))
    }
}

/// RFC 5424 section 6.3.3: inside PARAM-VALUE, `"`, `\` and `]` MUST be
/// escaped with a backslash.
pub fn escape_param_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '"' | '\\' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn push_param(out: &mut String, name: &str, value: &str) {
    out.push(' ');
    out.push_str(name);
    out.push_str("=\"");
    out.push_str(&escape_param_value(value));
    out.push('"');
}

/// Replace control characters (C0, DEL, C1) with `#` and three octal digits
/// of the code point, the same convention rsyslog uses (LF becomes `#012`).
/// This keeps every record on one line, so the file sink can use newline
/// delimiting and `grep` works.
fn escape_controls(s: &str) -> String {
    if !s.chars().any(char::is_control) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        if c.is_control() {
            let _ = write!(out, "#{:03o}", c as u32);
        } else {
            out.push(c);
        }
    }
    out
}
