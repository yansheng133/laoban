//! Formatting tests. All data is synthetic: documentation IP ranges
//! (192.0.2.0/24), `example.test` names and `local://u-test01` principals.
//! ID numbers get a valid check digit but belong to no one.

use laoban_audit::{
    escape_param_value, frame_octet_counting, pri, Action, AuditEvent, AuditFormatter,
    AuditTimestamp, ConfigError, Facility, FormatError, FormatterConfig, Outcome, Severity,
    TimestampError, DEFAULT_SD_ID,
};
use laoban_mask::{tw_id_checksum_valid, Masker, MaskerConfig};

/// Append the check digit that makes `prefix` (letter + 8 characters) valid.
fn with_check_digit(prefix: &str) -> String {
    for d in 0..10 {
        let candidate = format!("{prefix}{d}");
        if tw_id_checksum_valid(&candidate) {
            return candidate;
        }
    }
    panic!("no check digit for {prefix}");
}

fn masker() -> Masker {
    Masker::new(MaskerConfig {
        login_accounts: vec!["test01".into()],
        ..MaskerConfig::default()
    })
    .unwrap()
}

fn config() -> FormatterConfig {
    FormatterConfig {
        hostname: "app01.example.test".into(),
        app_name: "laoban".into(),
        procid: "4242".into(),
        ..FormatterConfig::default()
    }
}

fn formatter() -> AuditFormatter {
    AuditFormatter::new(config(), masker()).unwrap()
}

/// 2026-10-12T03:04:05.000120Z
fn ts() -> AuditTimestamp {
    AuditTimestamp::from_unix(1_791_774_245, 120).unwrap()
}

fn event(action: Action, outcome: Outcome) -> AuditEvent {
    AuditEvent::new(ts(), "local://u-test01", action, outcome)
}

#[test]
fn full_line_exact() {
    let ev = event(Action::DeploySucceeded, Outcome::Success)
        .project("demo")
        .target("cluster/test-a")
        .detail("deployed web to 192.0.2.10")
        .field("change_id", "c-0001");
    let rec = formatter().format(ev).unwrap();
    assert_eq!(
        rec.as_str(),
        "<133>1 2026-10-12T03:04:05.000120Z app01.example.test laoban 4242 deploy.succeeded \
         [laoban@32473 actor=\"local://u-test01\" action=\"deploy.succeeded\" outcome=\"success\" \
         project=\"demo\" target=\"cluster/test-a\" change_id=\"c-0001\" masked=\"0\"] \
         deployed web to 192.0.2.10"
    );
}

#[test]
fn nilvalue_for_empty_header_fields_and_no_msg_without_detail() {
    let f = AuditFormatter::new(
        FormatterConfig {
            hostname: String::new(),
            app_name: String::new(),
            procid: String::new(),
            ..FormatterConfig::default()
        },
        masker(),
    )
    .unwrap();
    let rec = f.format(event(Action::AiRequest, Outcome::Info)).unwrap();
    assert_eq!(
        rec.as_str(),
        "<134>1 2026-10-12T03:04:05.000120Z - - - ai.request \
         [laoban@32473 actor=\"local://u-test01\" action=\"ai.request\" outcome=\"info\" masked=\"0\"]"
    );
}

#[test]
fn timestamp_formatting() {
    assert_eq!(
        AuditTimestamp::from_unix(0, 0).unwrap().to_string(),
        "1970-01-01T00:00:00.000000Z"
    );
    assert_eq!(
        AuditTimestamp::from_unix(951_868_799, 999_999)
            .unwrap()
            .to_string(),
        "2000-02-29T23:59:59.999999Z"
    );
    assert_eq!(
        AuditTimestamp::from_unix(253_402_300_799, 0)
            .unwrap()
            .to_string(),
        "9999-12-31T23:59:59.000000Z"
    );
    assert_eq!(
        AuditTimestamp::from_unix(253_402_300_800, 0),
        Err(TimestampError::AfterYear9999)
    );
    assert_eq!(
        AuditTimestamp::from_unix(0, 1_000_000),
        Err(TimestampError::InvalidMicros(1_000_000))
    );
    let st = std::time::UNIX_EPOCH + std::time::Duration::new(1_791_774_245, 120_999);
    assert_eq!(AuditTimestamp::from_system_time(st).unwrap(), ts());
}

#[test]
fn pri_computation() {
    assert_eq!(pri(Facility::Kern, Severity::Emergency), 0);
    assert_eq!(pri(Facility::Local0, Severity::Notice), 133);
    assert_eq!(pri(Facility::LogAudit, Severity::Informational), 110);
    assert_eq!(pri(Facility::Local7, Severity::Debug), 191);

    let cases = [
        (Outcome::Success, "<133>"),
        (Outcome::Info, "<134>"),
        (Outcome::Rejected, "<132>"),
        (Outcome::Failure, "<131>"),
    ];
    for (outcome, prefix) in cases {
        let rec = formatter()
            .format(event(Action::ChangeSubmitted, outcome))
            .unwrap();
        assert!(rec.as_str().starts_with(prefix), "{outcome:?}: {rec}");
    }

    let f = AuditFormatter::new(
        FormatterConfig {
            facility: Facility::AuthPriv,
            ..config()
        },
        masker(),
    )
    .unwrap();
    let rec = f
        .format(event(Action::DeployFailed, Outcome::Failure))
        .unwrap();
    assert!(rec.as_str().starts_with("<83>1 "));
}

#[test]
fn sd_param_values_are_escaped() {
    assert_eq!(escape_param_value(r#"a"b\c]d"#), r#"a\"b\\c\]d"#);
    assert_eq!(escape_param_value("[ok] = fine"), "[ok\\] = fine");

    let ev = event(Action::SettingsChanged, Outcome::Success)
        .project(r#"say "hi""#)
        .target(r"C:\path]");
    let rec = formatter().format(ev).unwrap();
    assert!(rec.as_str().contains(r#"project="say \"hi\"""#), "{rec}");
    assert!(rec.as_str().contains(r#"target="C:\\path\]""#), "{rec}");
}

#[test]
fn control_characters_never_break_the_line() {
    let ev = event(Action::DeployFailed, Outcome::Failure)
        .detail("line one\nline two\r\ttab")
        .field("note", "a\nb");
    let rec = formatter().format(ev).unwrap();
    assert!(!rec.as_str().contains('\n') && !rec.as_str().contains('\r'));
    assert!(rec.as_str().ends_with("line one#012line two#015#011tab"));
    assert!(rec.as_str().contains(r#"note="a#012b""#));
}

#[test]
fn utf8_msg_gets_bom_ascii_does_not() {
    let rec = formatter()
        .format(event(Action::ChangeApproved, Outcome::Success).detail("核准部署"))
        .unwrap();
    assert!(rec.as_str().ends_with("] \u{FEFF}核准部署"), "{rec}");
    assert!(rec
        .as_bytes()
        .ends_with(b"] \xEF\xBB\xBF\xE6\xA0\xB8\xE5\x87\x86\xE9\x83\xA8\xE7\xBD\xB2"));

    let rec = formatter()
        .format(event(Action::ChangeApproved, Outcome::Success).detail("approved"))
        .unwrap();
    assert!(rec.as_str().ends_with("] approved"));
    assert!(!rec.as_str().contains('\u{FEFF}'));

    // SD values are UTF-8 without BOM (RFC 5424 section 6.3.3).
    let rec = formatter()
        .format(event(Action::ChangeApproved, Outcome::Success).project("測試專案"))
        .unwrap();
    assert!(rec.as_str().contains("project=\"測試專案\""));
    assert!(!rec.as_str().contains('\u{FEFF}'));
}

#[test]
fn octet_counting_uses_bytes_not_chars() {
    let rec = formatter()
        .format(event(Action::AiResponse, Outcome::Info).detail("叢集升級完成"))
        .unwrap();
    let framed = frame_octet_counting(&rec);
    let bytes = rec.as_bytes().len();
    assert!(bytes > rec.as_str().chars().count());
    let expected_prefix = format!("{bytes} ");
    assert!(framed.starts_with(expected_prefix.as_bytes()));
    assert_eq!(&framed[expected_prefix.len()..], rec.as_bytes());
    assert_eq!(framed.len(), expected_prefix.len() + bytes);
    assert_eq!(rec.len(), bytes);
}

#[test]
fn masking_is_applied_and_principal_kept() {
    let id = with_check_digit("F20481234");
    let ev = event(Action::ChangeSubmitted, Outcome::Info)
        .project(format!("proj-{id}"))
        .target(format!("cluster {id}"))
        .detail(format!("客戶身分證字號{id}，部署到 192.0.2.20"))
        .field("ticket", format!("申請人 {id}"));
    let rec = formatter().format(ev).unwrap();
    let s = rec.as_str();
    assert!(!s.contains(&id), "national ID leaked: {s}");
    assert!(s.contains("actor=\"local://u-test01\""));
    assert!(s.contains("客戶身分證字號[MASKED:TW_ID]，部署到 192.0.2.20"));
    // Inside SD values the token's `]` is escaped.
    assert!(s.contains("ticket=\"申請人 [MASKED:TW_ID\\]\""), "{s}");
    assert!(s.contains("masked=\"4\""), "{s}");
}

#[test]
fn login_accounts_are_kept_for_audit() {
    // Destination::Audit keeps login accounts (only AI masks them).
    let rec = formatter()
        .format(event(Action::SettingsChanged, Outcome::Success).detail("user test01 changed it"))
        .unwrap();
    assert!(rec.as_str().ends_with("user test01 changed it"));
}

#[test]
fn debug_of_event_hides_free_text() {
    let id = with_check_digit("A10000000");
    let ev = event(Action::AiRequest, Outcome::Info).detail(id.clone());
    let dbg = format!("{ev:?}");
    assert!(!dbg.contains(&id));
    assert!(dbg.contains("local://u-test01"));
}

type Setter = fn(&mut FormatterConfig, String);

#[test]
fn header_field_limits() {
    let cases: [(Setter, &str, usize); 3] = [
        (|c, v| c.hostname = v, "HOSTNAME", 255),
        (|c, v| c.app_name = v, "APP-NAME", 48),
        (|c, v| c.procid = v, "PROCID", 128),
    ];
    for (set, field, max) in cases {
        let mut ok = config();
        set(&mut ok, "a".repeat(max));
        assert!(
            AuditFormatter::new(ok, masker()).is_ok(),
            "{field} at limit"
        );

        let mut long = config();
        set(&mut long, "a".repeat(max + 1));
        assert_eq!(
            AuditFormatter::new(long, masker()).unwrap_err(),
            ConfigError::TooLong {
                field,
                len: max + 1,
                max
            }
        );

        for bad in ["has space", "tab\t", "測試", "del\u{7f}"] {
            let mut c = config();
            set(&mut c, bad.to_string());
            assert_eq!(
                AuditFormatter::new(c, masker()).unwrap_err(),
                ConfigError::NotPrintUsAscii { field },
                "{field} {bad:?}"
            );
        }
    }
}

#[test]
fn msgid_fits_limit_for_every_action() {
    for a in [
        Action::ChangeSubmitted,
        Action::ChangeApproved,
        Action::ChangeRejected,
        Action::DeployStarted,
        Action::DeploySucceeded,
        Action::DeployFailed,
        Action::AiRequest,
        Action::AiResponse,
        Action::BackupSkippedConfirmed,
        Action::SettingsChanged,
    ] {
        let s = a.as_str();
        assert!(s.len() <= laoban_audit::MSGID_MAX);
        assert!(s.bytes().all(|b| (33..=126).contains(&b)));
    }
}

#[test]
fn sd_id_validation() {
    assert_eq!(DEFAULT_SD_ID, "laoban@32473");
    let try_id = |id: &str| {
        AuditFormatter::new(
            FormatterConfig {
                sd_id: id.into(),
                ..config()
            },
            masker(),
        )
    };
    assert!(try_id("laoban@99999").is_ok());
    assert!(try_id(&format!("{}@1", "a".repeat(30))).is_ok()); // 32 chars
    for bad in [
        "",
        "laoban",
        "@32473",
        "laoban@",
        "laoban@abc",
        "a@b@1",
        "lao ban@1",
        "lao=ban@1",
        "lao]ban@1",
        "lao\"ban@1",
        "laoban@1.2",
    ] {
        assert!(
            matches!(try_id(bad), Err(ConfigError::InvalidSdId(_))),
            "{bad:?} should be rejected"
        );
    }
    assert!(try_id(&format!("{}@1", "a".repeat(31))).is_err()); // 33 chars

    let rec = try_id("audit@99999")
        .unwrap()
        .format(event(Action::AiRequest, Outcome::Info))
        .unwrap();
    assert!(rec.as_str().contains(" [audit@99999 actor="));
}

#[test]
fn field_name_validation() {
    let f = formatter();
    let fmt = |k: &str| f.format(event(Action::AiRequest, Outcome::Info).field(k, "v"));
    assert!(fmt("model").is_ok());
    assert!(fmt(&"k".repeat(32)).is_ok());
    for bad in ["", "a b", "a=b", "a]b", "a\"b", "a@1", "測試"] {
        assert_eq!(
            fmt(bad).unwrap_err(),
            FormatError::InvalidFieldName(bad.into())
        );
    }
    assert_eq!(
        fmt(&"k".repeat(33)).unwrap_err(),
        FormatError::InvalidFieldName("k".repeat(33))
    );
    for reserved in ["actor", "action", "outcome", "project", "target", "masked"] {
        assert_eq!(
            fmt(reserved).unwrap_err(),
            FormatError::ReservedFieldName(reserved.into())
        );
    }
    let dup = f.format(
        event(Action::AiRequest, Outcome::Info)
            .field("model", "a")
            .field("model", "b"),
    );
    assert_eq!(
        dup.unwrap_err(),
        FormatError::DuplicateFieldName("model".into())
    );
}
