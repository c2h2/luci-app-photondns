//! Minimal logger: stdout + optional file, with UTC timestamps and
//! size-capped truncation (routers have small /var).

use log::{Level, LevelFilter, Log, Metadata, Record};
use parking_lot::Mutex;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOG_SIZE: u64 = 4 * 1024 * 1024;

struct SimpleLogger {
    level: LevelFilter,
    file: Option<Mutex<(File, String)>>,
}

/// civil date from unix days (Howard Hinnant's algorithm)
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn timestamp() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let secs = now.as_secs() as i64;
    let (y, mo, d) = civil(secs.div_euclid(86400));
    let tod = secs.rem_euclid(86400);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        mo,
        d,
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

impl Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // keep our own logs + warnings from dependencies
        let target = record.target();
        if !target.starts_with("photondns") && record.level() > Level::Warn {
            return;
        }
        let line = format!("{} [{}] {}\n", timestamp(), record.level(), record.args());
        print!("{}", line);
        if let Some(file) = &self.file {
            let mut guard = file.lock();
            if let Ok(meta) = guard.0.metadata() {
                if meta.len() > MAX_LOG_SIZE {
                    let path = guard.1.clone();
                    if let Ok(f) = File::create(&path) {
                        guard.0 = f;
                    }
                }
            }
            let _ = guard.0.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

pub fn init(level: &str, file: &str) -> anyhow::Result<()> {
    let level = match level {
        "debug" => LevelFilter::Debug,
        "warn" => LevelFilter::Warn,
        "error" => LevelFilter::Error,
        _ => LevelFilter::Info,
    };
    let file_handle = if file.is_empty() {
        None
    } else {
        match OpenOptions::new().create(true).append(true).open(file) {
            Ok(f) => Some(Mutex::new((f, file.to_string()))),
            Err(e) => {
                eprintln!("cannot open log file {}: {}", file, e);
                None
            }
        }
    };
    log::set_boxed_logger(Box::new(SimpleLogger {
        level,
        file: file_handle,
    }))?;
    log::set_max_level(level);
    Ok(())
}
