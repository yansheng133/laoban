//! Audit records for laoban.
//!
//! * [`AuditEvent`] describes what happened, who did it (a Rancher principal
//!   ID) and the outcome.
//! * [`AuditFormatter`] masks every free-text value with
//!   [`laoban_mask::Masker`] ([`laoban_mask::Destination::Audit`]) and renders
//!   the event as an RFC 5424 syslog message. The formatter owns the masker,
//!   so an event cannot be formatted without masking.
//! * [`frame_octet_counting`] applies RFC 5425 octet-counting framing for
//!   transport over TLS.
//! * [`FileSink`] writes records to a local file with size-based rotation.
//!
//! This crate has no network code. The TLS transport is a separate concern.

mod event;
mod sink;
mod syslog;
mod timestamp;

pub use event::{Action, AuditEvent, Outcome};
pub use sink::{FileSink, RotationConfig};
pub use syslog::{
    escape_param_value, frame_octet_counting, pri, AuditFormatter, ConfigError, Facility,
    FormatError, FormatterConfig, Severity, SyslogRecord, APP_NAME_MAX, DEFAULT_SD_ID,
    HOSTNAME_MAX, MSGID_MAX, NILVALUE, PROCID_MAX, RESERVED_PARAM_NAMES, SD_NAME_MAX,
};
pub use timestamp::{AuditTimestamp, TimestampError};
