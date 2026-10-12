//! Local file sink with size-based rotation.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::syslog::SyslogRecord;

/// Rotation settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RotationConfig {
    /// Size limit of the active file in bytes, including the trailing LF of
    /// each record. Must be at least 1.
    pub max_bytes: u64,
    /// Number of rotated files kept (`audit.log.1` .. `audit.log.N`), not
    /// counting the active file. Must be at least 1. When rotation would
    /// create `audit.log.N+1`, the oldest file is deleted instead, so the
    /// external archiving job must collect rotated files before then.
    pub max_files: u32,
}

/// Appends records to a file, one record per line.
///
/// Format: each line is one RFC 5424 `SYSLOG-MSG` followed by `\n` (LF).
/// This is newline-delimited, not RFC 5425 octet-counted; the formatter
/// guarantees records contain no CR/LF. To ship a line over TLS, strip the
/// LF and pass the record through [`crate::frame_octet_counting`].
///
/// Rotation: before a write, if the active file is not empty and the record
/// would push it past `max_bytes`, the files are shifted
/// (`audit.log.1` -> `audit.log.2`, ..., `audit.log` -> `audit.log.1`) and a
/// new `audit.log` is started. A record is never split across files. A record
/// that alone exceeds `max_bytes` is still written, alone, to a fresh file
/// (an audit record is never dropped); that file is rotated away on the next
/// write.
///
/// One `FileSink` per path; it is not safe for several processes or sinks
/// to write the same file.
#[derive(Debug)]
pub struct FileSink {
    path: PathBuf,
    config: RotationConfig,
    file: File,
    size: u64,
}

impl FileSink {
    /// Open (or create) `path` for appending.
    pub fn open(path: impl AsRef<Path>, config: RotationConfig) -> io::Result<Self> {
        if config.max_bytes == 0 || config.max_files == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_bytes and max_files must both be at least 1",
            ));
        }
        let path = path.as_ref().to_path_buf();
        let file = open_append(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            config,
            file,
            size,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Current size of the active file in bytes.
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Append one record plus LF, rotating first if needed.
    pub fn write(&mut self, record: &SyslogRecord) -> io::Result<()> {
        let mut line = Vec::with_capacity(record.len() + 1);
        line.extend_from_slice(record.as_bytes());
        line.push(b'\n');
        let len = line.len() as u64;

        if self.size > 0 && self.size.saturating_add(len) > self.config.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(&line)?;
        self.size += len;
        Ok(())
    }

    /// Flush file contents to stable storage (`fsync` data).
    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_data()
    }

    fn rotated(&self, n: u32) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.sync_data()?;
        let max = self.config.max_files;
        let oldest = self.rotated(max);
        match fs::remove_file(&oldest) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        for n in (1..max).rev() {
            let from = self.rotated(n);
            if from.exists() {
                fs::rename(&from, self.rotated(n + 1))?;
            }
        }
        fs::rename(&self.path, self.rotated(1))?;
        self.file = open_append(&self.path)?;
        self.size = 0;
        Ok(())
    }
}

fn open_append(path: &Path) -> io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Owner read/write, group read. Only applies when the file is created.
        opts.mode(0o640);
    }
    opts.open(path)
}
