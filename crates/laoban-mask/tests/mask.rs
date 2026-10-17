//! All test data is synthetic. ID numbers are generated with a valid check
//! digit so the checksum path is exercised, but they belong to no one.

use laoban_mask::{
    luhn_valid, tw_id_checksum_valid, CustomRule, Destination, Kind, MaskError, Masker,
    MaskerConfig,
};

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
        login_accounts: vec!["sam".into(), "ops.admin@example.com".into()],
        ..MaskerConfig::default()
    })
    .unwrap()
}

#[test]
fn check_digit_reference_value() {
    // The textbook example used in Taiwan ID validation tutorials.
    assert!(tw_id_checksum_valid("A123456789"));
    assert!(!tw_id_checksum_valid("A123456788"));
}

#[test]
fn national_id_in_chinese_text_is_masked_for_both_destinations() {
    let id = with_check_digit("F20481234");
    let text = format!("客戶身分證字號{id}，請協助查詢。");
    for dest in [Destination::Ai, Destination::Audit] {
        let out = masker().mask(&text, dest);
        assert_eq!(out.text, "客戶身分證字號[MASKED:TW_ID]，請協助查詢。");
        assert_eq!(out.findings.len(), 1);
        assert_eq!(out.findings[0].kind, Kind::TwNationalId);
        assert_eq!(out.findings[0].checksum_valid, Some(true));
    }
}

#[test]
fn invalid_check_digit_is_still_masked() {
    let valid = with_check_digit("B11111111");
    let last = valid.chars().last().unwrap().to_digit(10).unwrap();
    let invalid = format!("{}{}", &valid[..9], (last + 1) % 10);
    assert!(!tw_id_checksum_valid(&invalid));
    let out = masker().mask(&invalid, Destination::Ai);
    assert_eq!(out.text, "[MASKED:TW_ID]");
    assert_eq!(out.findings[0].checksum_valid, Some(false));
}

#[test]
fn id_glued_to_other_alphanumerics_is_not_masked() {
    let id = with_check_digit("A12345678");
    for text in [format!("X{id}"), format!("{id}0"), format!("sku{id}")] {
        let out = masker().mask(&text, Destination::Ai);
        assert!(out.findings.is_empty(), "unexpected finding in {text}");
        assert_eq!(out.text, text);
    }
}

#[test]
fn resident_certificate_formats_are_masked() {
    let new_format = with_check_digit("A80000001");
    let old_format = with_check_digit("AC0000001");
    let text = format!("居留證 {new_format} 與舊式 {old_format}");
    let out = masker().mask(&text, Destination::Audit);
    assert_eq!(
        out.text,
        "居留證 [MASKED:TW_RESIDENT_ID] 與舊式 [MASKED:TW_RESIDENT_ID]"
    );
    assert!(out.findings.iter().all(|f| f.kind == Kind::TwResidentId));
    assert!(out.findings.iter().all(|f| f.checksum_valid == Some(true)));
}

#[test]
fn payment_card_with_valid_luhn_is_masked() {
    assert!(luhn_valid("4111111111111111"));
    for text in [
        "卡號 4111 1111 1111 1111 已停用",
        "card=4111-1111-1111-1111;",
        "4111111111111111",
    ] {
        let out = masker().mask(text, Destination::Ai);
        assert!(out.text.contains("[MASKED:CARD]"), "not masked: {text}");
        assert!(!out.text.contains("4111"), "leaked: {}", out.text);
    }
}

#[test]
fn digit_run_failing_luhn_is_not_masked_as_card() {
    let out = masker().mask("訂單 4111111111111112", Destination::Ai);
    assert!(out.findings.iter().all(|f| f.kind != Kind::PaymentCard));
}

#[test]
fn card_inside_longer_digit_run_is_not_masked() {
    // 4111111111111111 followed by more digits is not a card number.
    let out = masker().mask("trace 41111111111111110000000", Destination::Ai);
    assert!(out.findings.iter().all(|f| f.kind != Kind::PaymentCard));
}

#[test]
fn financial_account_after_keyword_is_masked_but_keyword_kept() {
    let cases = [
        ("轉入帳號：012-345678901", "轉入帳號：[MASKED:ACCOUNT]"),
        ("存款帳號 0123456789012", "存款帳號 [MASKED:ACCOUNT]"),
        ("Account No. 1234567890", "Account No. [MASKED:ACCOUNT]"),
        ("acct# 9876543210", "acct# [MASKED:ACCOUNT]"),
    ];
    for (input, expected) in cases {
        let out = masker().mask(input, Destination::Audit);
        assert_eq!(out.text, expected, "input: {input}");
    }
}

#[test]
fn short_number_after_account_keyword_is_not_masked() {
    let out = masker().mask("帳號 12345", Destination::Ai);
    assert!(out.findings.is_empty());
}

#[test]
fn login_accounts_are_masked_only_for_ai() {
    let text = "請把 sam 和 OPS.ADMIN@example.com 加進 Project";
    let ai = masker().mask(text, Destination::Ai);
    assert_eq!(
        ai.text,
        "請把 [MASKED:LOGIN] 和 [MASKED:LOGIN] 加進 Project"
    );
    let audit = masker().mask(text, Destination::Audit);
    assert_eq!(audit.text, text);
    assert!(audit.findings.is_empty());
}

#[test]
fn login_account_inside_another_name_is_not_masked() {
    for text in ["samuel 登入了", "sam.lin 登入了", "user_sam_2 登入了"] {
        let out = masker().mask(text, Destination::Ai);
        assert!(out.findings.is_empty(), "unexpected finding in {text}");
    }
}

#[test]
fn ip_addresses_are_never_masked() {
    let text = "服務在 192.0.2.10:8443，另一個在 10.42.0.15 與 fd00::1";
    for dest in [Destination::Ai, Destination::Audit] {
        let out = masker().mask(text, dest);
        assert_eq!(out.text, text);
        assert!(out.findings.is_empty());
    }
}

#[test]
fn custom_rule_applies_to_both_destinations() {
    let m = Masker::new(MaskerConfig {
        custom_rules: vec![CustomRule {
            name: "employee id".into(),
            pattern: r"EMP-\d{6}".into(),
            case_insensitive: true,
        }],
        ..MaskerConfig::default()
    })
    .unwrap();
    for dest in [Destination::Ai, Destination::Audit] {
        let out = m.mask("負責人 emp-004321", dest);
        assert_eq!(out.text, "負責人 [MASKED:CUSTOM_EMPLOYEE_ID]");
        assert_eq!(out.findings[0].kind, Kind::Custom("employee id".into()));
    }
}

#[test]
fn invalid_custom_rule_is_rejected() {
    let err = Masker::new(MaskerConfig {
        custom_rules: vec![CustomRule {
            name: "broken".into(),
            pattern: "(".into(),
            case_insensitive: false,
        }],
        ..MaskerConfig::default()
    })
    .unwrap_err();
    assert!(matches!(err, MaskError::InvalidCustomRule { .. }));
}

#[test]
fn custom_rule_matching_empty_string_is_rejected() {
    let err = Masker::new(MaskerConfig {
        custom_rules: vec![CustomRule {
            name: "everything".into(),
            pattern: "x*".into(),
            case_insensitive: false,
        }],
        ..MaskerConfig::default()
    })
    .unwrap_err();
    assert!(matches!(err, MaskError::EmptyMatchingRule { .. }));
}

#[test]
fn overlapping_findings_produce_a_single_token() {
    let id = with_check_digit("A12345678");
    let m = Masker::new(MaskerConfig {
        custom_rules: vec![CustomRule {
            name: "starts with A1".into(),
            pattern: r"A1\d{3}".into(),
            case_insensitive: false,
        }],
        ..MaskerConfig::default()
    })
    .unwrap();
    let out = m.mask(&format!("ID {id} end"), Destination::Audit);
    assert_eq!(out.findings.len(), 1);
    assert_eq!(out.text.matches("[MASKED:").count(), 1);
    // The custom rule alone only covers the first five characters, so the
    // last five must also be gone: the ID detector has to have fired too.
    assert!(
        !out.text.contains(&id[5..]),
        "tail of ID leaked: {}",
        out.text
    );
    assert!(
        out.findings[0].end - out.findings[0].start >= id.len(),
        "masked span shorter than the ID"
    );
}

#[test]
fn findings_never_contain_the_original_value() {
    let id = with_check_digit("Z29999999");
    let text = format!("{id} / 4111 1111 1111 1111 / 帳號 1234567890 / sam");
    let out = masker().mask(&text, Destination::Ai);
    let json = serde_json::to_string(&out.findings).unwrap();
    for secret in [id.as_str(), "4111", "1234567890", "sam"] {
        assert!(!json.contains(secret), "findings leaked {secret}: {json}");
        assert!(
            !out.text.contains(secret),
            "text leaked {secret}: {}",
            out.text
        );
    }
}

#[test]
fn disabled_categories_are_left_alone() {
    let m = Masker::new(MaskerConfig {
        mask_national_ids: false,
        mask_financial_accounts: false,
        mask_login_accounts_for_ai: false,
        mask_credentials: false,
        login_accounts: vec!["sam".into()],
        custom_rules: vec![],
    })
    .unwrap();
    let id = with_check_digit("A12345678");
    let text = format!("{id} 4111111111111111 sam");
    assert_eq!(m.mask(&text, Destination::Ai).text, text);
}

#[test]
fn credentials_are_masked_for_both_destinations() {
    // Synthetic values in the providers' formats, assembled at run time so
    // secret scanners do not mistake the test for a leak. None is real.
    let anthropic = format!("{}-ant-api03-{}", "sk", "x".repeat(24));
    let github = format!("{}_{}", "ghp", "A1b2".repeat(9));
    let pat = format!("{}_pat_{}", "github", "Z9_".repeat(10));
    let aws = format!("{}{}", "AKIA", "EXAMPLE".repeat(2) + "00");
    let aws = aws.as_str();
    let jwt = [
        "eyJ",
        &"a".repeat(12),
        ".",
        "eyJ",
        &"b".repeat(12),
        ".",
        &"c".repeat(12),
    ]
    .concat();
    let jwt = jwt.as_str();
    let kind = "OPENSSH PRIVATE KEY";
    let pem = format!("-----BEGIN {kind}-----\nAAAAexample\n-----END {kind}-----");
    let bearer = "q".repeat(32);
    let pw = ["Sup3r", "S3cret!"].concat();
    let pw2 = ["hunter", "22"].concat();
    let key = ["k-", "1234567"].concat();
    let text = format!(
        "key {anthropic}, gh {github}, pat {pat}, aws {aws}, jwt {jwt}\n{pem}\n\
         Authorization: Bearer {bearer}\n\
         db password={pw} and 資料庫密碼：{pw2}，api_key: \"{key}\""
    );
    for dest in [Destination::Ai, Destination::Audit] {
        let out = masker().mask(&text, dest);
        for secret in [
            anthropic.as_str(),
            github.as_str(),
            pat.as_str(),
            aws,
            jwt,
            "AAAAexample",
            bearer.as_str(),
            pw.as_str(),
            pw2.as_str(),
            key.as_str(),
        ] {
            assert!(!out.text.contains(secret), "{secret} leaked: {}", out.text);
        }
        assert!(out.findings.iter().all(|f| f.kind == Kind::Credential));
        assert!(
            out.text.contains("db password=[MASKED:SECRET]"),
            "{}",
            out.text
        );
        assert!(
            out.text.contains("資料庫密碼：[MASKED:SECRET]"),
            "{}",
            out.text
        );
    }
}

#[test]
fn ordinary_words_are_not_credentials() {
    let text = "做一個登入頁，要有密碼欄位與忘記密碼連結；token 用量顯示在右上角。";
    let out = masker().mask(text, Destination::Ai);
    assert_eq!(out.text, text);
    assert!(out.findings.is_empty());
}

#[test]
fn credential_masking_can_be_turned_off() {
    let m = Masker::new(MaskerConfig {
        mask_credentials: false,
        ..MaskerConfig::default()
    })
    .unwrap();
    let text = ["password=", "plain-value"].concat();
    assert_eq!(m.mask(&text, Destination::Ai).text, text);
}
