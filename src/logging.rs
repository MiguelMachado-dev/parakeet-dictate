//! The log file: dictate.log beside the app, rotated at 1 MB with two backups. It holds lengths and timings,
//! never dictated text, keys, or tokens.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError};

use log::{Level, LevelFilter, Log, Metadata, Record};
use windows_sys::Win32::Foundation::SYSTEMTIME;
use windows_sys::Win32::System::SystemInformation::GetLocalTime;

use crate::paths;

const MAX_BYTES: u64 = 1_000_000;
const BACKUPS: u32 = 2;

struct FileLog {
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl FileLog {
    fn backup(&self, index: u32) -> PathBuf {
        paths::temporary_beside(&self.path, &format!(".{index}"))
    }

    fn open(&self) -> Option<File> {
        OpenOptions::new().create(true).append(true).open(&self.path).ok()
    }

    fn rotate(&self, file: &mut Option<File>) {
        *file = None;
        for index in (1..BACKUPS).rev() {
            let _ = fs::rename(self.backup(index), self.backup(index + 1));
        }
        let _ = fs::rename(&self.path, self.backup(1));
        *file = self.open();
    }
}

fn timestamp() -> String {
    let mut now = SYSTEMTIME { wYear: 0, wMonth: 0, wDayOfWeek: 0, wDay: 0, wHour: 0, wMinute: 0, wSecond: 0, wMilliseconds: 0 };
    unsafe { GetLocalTime(&mut now) };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02},{:03}",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds
    )
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Error => "ERROR",
        Level::Warn => "WARNING",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    }
}

impl Log for FileLog {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let ours = metadata.target().starts_with("parakeet");
        metadata.level() <= if ours { Level::Info } else { Level::Warn }
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let thread = std::thread::current();
        let line = format!(
            "{} {} {} {}: {}\n",
            timestamp(),
            level_name(record.level()),
            thread.name().unwrap_or("worker"),
            record.target().replace("::", "."),
            record.args(),
        );
        let mut file = self.file.lock().unwrap_or_else(PoisonError::into_inner);
        if file.is_none() {
            *file = self.open();
        }
        let size = file.as_ref().and_then(|file| file.metadata().ok()).map_or(0, |metadata| metadata.len());
        if size > 0 && size + line.len() as u64 >= MAX_BYTES {
            self.rotate(&mut file);
        }
        if let Some(file) = file.as_mut() {
            let _ = file.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {}
}

pub fn setup() {
    let logger = Box::new(FileLog { path: paths::log(), file: Mutex::new(None) });
    if log::set_boxed_logger(logger).is_ok() {
        log::set_max_level(LevelFilter::Info);
    }
    std::panic::set_hook(Box::new(|panic| {
        let thread = std::thread::current();
        let location = panic.location().map_or_else(String::new, ToString::to_string);
        log::error!(target: "parakeet_dictate", "uncaught panic in {} at {location}", thread.name().unwrap_or("worker"));
    }));
}
