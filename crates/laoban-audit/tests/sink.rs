//! File sink rotation tests. Synthetic data only.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use laoban_audit::{
    Action, AuditEvent, AuditFormatter, AuditTimestamp, FileSink, FormatterConfig, Outcome,
    RotationConfig, SyslogRecord,
};
use laoban_mask::{Masker, MaskerConfig};

/// Unique directory under the system temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "laoban-audit-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn formatter() -> AuditFormatter {
    AuditFormatter::new(
        FormatterConfig {
            hostname: "app01.example.test".into(),
            ..FormatterConfig::default()
        },
        Masker::new(MaskerConfig::default()).unwrap(),
    )
    .unwrap()
}

fn record(i: u32, detail: &str) -> SyslogRecord {
    let ts = AuditTimestamp::from_unix(1_791_774_245 + u64::from(i), 0).unwrap();
    formatter()
        .format(
            AuditEvent::new(ts, "local://u-test01", Action::DeployStarted, Outcome::Info)
                .field("seq", i.to_string())
                .detail(detail),
        )
        .unwrap()
}

fn lines(p: &Path) -> Vec<String> {
    match fs::read_to_string(p) {
        Ok(s) => {
            assert!(s.is_empty() || s.ends_with('\n'), "partial record in {p:?}");
            s.lines().map(str::to_string).collect()
        }
        Err(_) => Vec::new(),
    }
}

fn rotated(base: &Path, n: u32) -> PathBuf {
    PathBuf::from(format!("{}.{n}", base.display()))
}

#[test]
fn writes_newline_delimited_records() {
    let dir = TempDir::new("nl");
    let log = dir.path().join("audit.log");
    let mut sink = FileSink::open(
        &log,
        RotationConfig {
            max_bytes: 1 << 20,
            max_files: 3,
        },
    )
    .unwrap();
    let a = record(1, "first");
    let b = record(2, "第二筆");
    sink.write(&a).unwrap();
    sink.write(&b).unwrap();
    sink.sync().unwrap();
    assert_eq!(lines(&log), vec![a.as_str(), b.as_str()]);
    assert_eq!(sink.size(), (a.len() + b.len() + 2) as u64);
}

#[test]
fn rotation_shifts_files_and_keeps_max_files() {
    let dir = TempDir::new("rot");
    let log = dir.path().join("audit.log");
    let recs: Vec<SyslogRecord> = (0..10).map(|i| record(i, "rotate me")).collect();
    let line_len = recs[0].len() as u64 + 1;
    assert!(recs.iter().all(|r| r.len() as u64 + 1 == line_len));

    // Two records fit per file; the third forces rotation.
    let mut sink = FileSink::open(
        &log,
        RotationConfig {
            max_bytes: line_len * 2 + 1,
            max_files: 2,
        },
    )
    .unwrap();
    for r in &recs {
        sink.write(r).unwrap();
    }
    drop(sink);

    // 10 records, 2 per file: active has 8,9; .1 has 6,7; .2 has 4,5; older dropped.
    let s = |i: usize| recs[i].as_str().to_string();
    assert_eq!(lines(&log), vec![s(8), s(9)]);
    assert_eq!(lines(&rotated(&log, 1)), vec![s(6), s(7)]);
    assert_eq!(lines(&rotated(&log, 2)), vec![s(4), s(5)]);
    assert!(!rotated(&log, 3).exists());
    for p in [log.clone(), rotated(&log, 1), rotated(&log, 2)] {
        assert!(fs::metadata(&p).unwrap().len() <= line_len * 2 + 1);
    }
}

#[test]
fn record_is_never_split_across_files() {
    let dir = TempDir::new("split");
    let log = dir.path().join("audit.log");
    let small = record(1, "x");
    let big = record(2, &"大".repeat(40));
    // Room for `small` plus part of `big`, but not all of it.
    let max = (small.len() + 1 + big.len()) as u64;
    let mut sink = FileSink::open(
        &log,
        RotationConfig {
            max_bytes: max,
            max_files: 5,
        },
    )
    .unwrap();
    sink.write(&small).unwrap();
    sink.write(&big).unwrap();
    drop(sink);
    assert_eq!(lines(&rotated(&log, 1)), vec![small.as_str()]);
    assert_eq!(lines(&log), vec![big.as_str()]);
}

#[test]
fn oversized_record_is_written_to_a_fresh_file() {
    let dir = TempDir::new("big");
    let log = dir.path().join("audit.log");
    let a = record(1, "a");
    let huge = record(2, &"h".repeat(500));
    let b = record(3, "b");
    let max = 200;
    assert!(huge.len() as u64 > max);
    let mut sink = FileSink::open(
        &log,
        RotationConfig {
            max_bytes: max,
            max_files: 5,
        },
    )
    .unwrap();
    sink.write(&a).unwrap();
    sink.write(&huge).unwrap();
    sink.write(&b).unwrap();
    drop(sink);
    assert_eq!(lines(&rotated(&log, 2)), vec![a.as_str()]);
    assert_eq!(lines(&rotated(&log, 1)), vec![huge.as_str()]);
    assert_eq!(lines(&log), vec![b.as_str()]);
}

#[test]
fn oversized_first_record_goes_into_empty_active_file() {
    let dir = TempDir::new("big1");
    let log = dir.path().join("audit.log");
    let huge = record(1, &"h".repeat(500));
    let mut sink = FileSink::open(
        &log,
        RotationConfig {
            max_bytes: 10,
            max_files: 1,
        },
    )
    .unwrap();
    sink.write(&huge).unwrap();
    drop(sink);
    assert_eq!(lines(&log), vec![huge.as_str()]);
    assert!(!rotated(&log, 1).exists());
}

#[test]
fn reopening_appends_and_counts_existing_size() {
    let dir = TempDir::new("reopen");
    let log = dir.path().join("audit.log");
    let cfg = RotationConfig {
        max_bytes: 1 << 20,
        max_files: 1,
    };
    let a = record(1, "a");
    let b = record(2, "b");
    FileSink::open(&log, cfg).unwrap().write(&a).unwrap();
    let mut sink = FileSink::open(&log, cfg).unwrap();
    assert_eq!(sink.size(), a.len() as u64 + 1);
    sink.write(&b).unwrap();
    drop(sink);
    assert_eq!(lines(&log), vec![a.as_str(), b.as_str()]);
}

#[test]
fn zero_limits_are_rejected() {
    let dir = TempDir::new("zero");
    let log = dir.path().join("audit.log");
    for (max_bytes, max_files) in [(0, 1), (1, 0)] {
        let err = FileSink::open(
            &log,
            RotationConfig {
                max_bytes,
                max_files,
            },
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }
}
