//! Logging: colored console output plus a rotating log file.
//!
//! Rust port of `backend/utils/logger.py`.
//!
//! The Python project configured logging with a
//! `logging.Formatter`-style template, for example:
//!
//! ```text
//! [%(asctime)s %(levelname)s ] [%(filename)s:%(lineno)d %(module)s.%(funcName)s] %(message)s
//! ```
//!
//! `server.log.format` is still honored: the placeholder tokens are translated
//! into a [`FormatEvent`] implementation, so existing configuration files keep
//! working. Rotation is time based (`Rotation::DAILY`) with
//! `server.log.backupCount` files kept; see `docs/MIGRATION.md` for why
//! `server.log.maxBytes` no longer applies.

use std::sync::OnceLock;

use tracing::{Event, Subscriber};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use crate::config::Config;

/// Keeps the non-blocking file writer alive for the lifetime of the process.
pub struct LoggerGuard {
    _file_guard: tracing_appender::non_blocking::WorkerGuard,
}

static INITIALIZED: OnceLock<()> = OnceLock::new();

/// Default template, identical to the one shipped in `config.yaml`.
pub const DEFAULT_FORMAT: &str =
    "[%(asctime)s %(levelname)s ] [%(filename)s:%(lineno)d %(module)s.%(funcName)s] %(message)s";

/// Initialize the global logger from `server.log.*` configuration.
///
/// Safe to call once; later calls are ignored.
pub fn init(config: &Config) -> anyhow::Result<LoggerGuard> {
    let level = config.string("server.log.level", "INFO");
    let level = if config.bool("server.log.debug", false) {
        "DEBUG".to_string()
    } else {
        level
    };
    let template = config.string("server.log.format", DEFAULT_FORMAT);
    let file_path = config.string("server.log.filename", "logs/run.log");
    let backup_count = config.i64("server.log.backupCount", 10).max(1) as usize;

    let (directory, prefix, suffix) = split_log_path(&file_path);
    std::fs::create_dir_all(&directory)?;

    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix(suffix)
        .max_log_files(backup_count)
        .build(&directory)?;
    let (non_blocking, file_guard) = tracing_appender::non_blocking(file_appender);

    let console_formatter = TemplateFormatter::new(&template, true);
    let file_formatter = TemplateFormatter::new(&template, false);

    let console_layer = tracing_subscriber::fmt::layer()
        .event_format(console_formatter)
        .with_writer(std::io::stdout);

    let file_layer = tracing_subscriber::fmt::layer()
        .event_format(file_formatter)
        .with_ansi(false)
        .with_writer(non_blocking);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{level},sqlx=warn,hyper=warn")));

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .with(file_layer);

    if INITIALIZED.set(()).is_ok() {
        subscriber.init();
    }

    Ok(LoggerGuard {
        _file_guard: file_guard,
    })
}

fn split_log_path(path: &str) -> (String, String, String) {
    let path = std::path::Path::new(path);
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_else(|| ".".to_string());
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "run.log".to_string());
    match file_name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => {
            (directory, stem.to_string(), extension.to_string())
        }
        _ => (directory, file_name, "log".to_string()),
    }
}

#[derive(Debug, Clone)]
enum Segment {
    Literal(String),
    Time,
    Level,
    File,
    Line,
    Module,
    Function,
    Target,
    Message,
}

/// Formats events using the Python-style `%(...)s` template.
#[derive(Debug, Clone)]
pub struct TemplateFormatter {
    segments: Vec<Segment>,
    ansi: bool,
}

impl TemplateFormatter {
    /// Parse a Python logging template.
    pub fn new(template: &str, ansi: bool) -> Self {
        Self {
            segments: parse_template(template),
            ansi,
        }
    }
}

fn parse_template(template: &str) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("%(") {
        if start > 0 {
            segments.push(Segment::Literal(rest[..start].to_string()));
        }
        let after = &rest[start + 2..];
        let Some(end) = after.find(')') else {
            segments.push(Segment::Literal(rest[start..].to_string()));
            return segments;
        };
        let name = &after[..end];
        let mut consumed = start + 2 + end + 1;
        // skip the trailing conversion character (`s`, `d`, ...)
        if let Some(conversion) = rest[consumed..].chars().next() {
            if conversion.is_ascii_alphabetic() {
                consumed += conversion.len_utf8();
            }
        }
        let segment = match name {
            "asctime" => Segment::Time,
            "levelname" => Segment::Level,
            "filename" => Segment::File,
            "lineno" => Segment::Line,
            "module" => Segment::Module,
            "funcName" => Segment::Function,
            "name" => Segment::Target,
            "message" => Segment::Message,
            other => Segment::Literal(format!("%({other})")),
        };
        segments.push(segment);
        rest = &rest[consumed..];
    }
    if !rest.is_empty() {
        segments.push(Segment::Literal(rest.to_string()));
    }
    segments
}

impl<S, N> FormatEvent<S, N> for TemplateFormatter
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> std::fmt::Result {
        let metadata = event.metadata();
        let ansi = self.ansi && writer.has_ansi_escapes();
        for segment in &self.segments {
            match segment {
                Segment::Literal(text) => writer.write_str(text)?,
                Segment::Time => write!(
                    writer,
                    "{}",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
                )?,
                Segment::Level => {
                    let level = *metadata.level();
                    if ansi {
                        write!(writer, "{}", colorize(level, level.as_str()))?;
                    } else {
                        write!(writer, "{level}")?;
                    }
                }
                Segment::File => writer.write_str(metadata.file().unwrap_or("?"))?,
                Segment::Line => write!(writer, "{}", metadata.line().unwrap_or(0))?,
                Segment::Module => writer.write_str(metadata.module_path().unwrap_or("?"))?,
                Segment::Function => {
                    // `tracing` has no function name in its metadata: the
                    // default event name is `"event <file>:<line>"`.
                    writer.write_str(short_event_name(metadata.name()))?
                }
                Segment::Target => writer.write_str(metadata.target())?,
                Segment::Message => ctx.field_format().format_fields(writer.by_ref(), event)?,
            }
        }
        writeln!(writer)
    }
}

/// Strip the `"event <file>:<line>"` suffix from a `tracing` event name.
fn short_event_name(name: &str) -> &str {
    name.split_whitespace().next().unwrap_or(name)
}

fn colorize(level: tracing::Level, text: &str) -> String {
    let color = match level {
        tracing::Level::TRACE => "\x1b[90m",
        tracing::Level::DEBUG => "\x1b[37m",
        tracing::Level::INFO => "\x1b[32m",
        tracing::Level::WARN => "\x1b[33m",
        tracing::Level::ERROR => "\x1b[1;31m",
    };
    format!("{color}{text}\x1b[0m")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_split_into_segments() {
        let segments = parse_template("[%(asctime)s %(levelname)s] %(message)s");
        assert!(matches!(segments[0], Segment::Literal(ref s) if s == "["));
        assert!(matches!(segments[1], Segment::Time));
        assert!(matches!(segments[2], Segment::Literal(ref s) if s == " "));
        assert!(matches!(segments[3], Segment::Level));
        assert!(matches!(segments[4], Segment::Literal(ref s) if s == "] "));
        assert!(matches!(segments[5], Segment::Message));
    }

    #[test]
    fn unknown_placeholders_are_kept_as_text() {
        let segments = parse_template("%(nope)s done");
        assert!(matches!(segments[0], Segment::Literal(ref s) if s == "%(nope)"));
    }

    #[test]
    fn tracing_event_names_are_shortened() {
        assert_eq!(short_event_name("event src/app.rs:47"), "event");
        assert_eq!(short_event_name("request"), "request");
    }

    #[test]
    fn log_paths_are_split_for_rotation() {
        let (dir, prefix, suffix) = split_log_path("logs/run.log");
        assert_eq!(dir, "logs");
        assert_eq!(prefix, "run");
        assert_eq!(suffix, "log");

        let (dir, prefix, suffix) = split_log_path("run");
        assert_eq!(dir, ".");
        assert_eq!(prefix, "run");
        assert_eq!(suffix, "log");
    }
}
