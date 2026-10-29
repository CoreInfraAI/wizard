use file_rotate::{ContentLimit, FileRotate, compression::Compression, suffix::AppendCount};
use log::{LevelFilter, Log, Metadata, Record};
use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    sync::{Mutex, OnceLock},
};

static LOGGER: OnceLock<Logger> = OnceLock::new();

struct Logger {
    output: Mutex<FileRotate<AppendCount>>,
}

pub(crate) fn initialize() -> Result<(), &'static str> {
    let level = match std::env::var("WIZARD_LOG") {
        Ok(value) => value.parse().map_err(|_| "invalid WIZARD_LOG level")?,
        Err(std::env::VarError::NotPresent) => LevelFilter::Info,
        Err(std::env::VarError::NotUnicode(_)) => return Err("invalid WIZARD_LOG encoding"),
    };
    let directory =
        wizard_settings::paths::logs_dir().map_err(|_| "failed to resolve proxy log directory")?;
    fs::create_dir_all(&directory)
        .inspect_err(|error| report_stderr("proxy log directory creation", error))
        .map_err(|_| "failed to create proxy log directory")?;
    let mut options = OpenOptions::new();
    options.read(true).create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let path = directory.join("Proxy.log");
    // FileRotate suppresses open errors; report initialization failures before using it.
    drop(
        options
            .open(&path)
            .inspect_err(|error| report_stderr("proxy log file open", error))
            .map_err(|_| "failed to open proxy log file")?,
    );
    let output = FileRotate::new(
        path,
        AppendCount::new(5),
        ContentLimit::BytesSurpassed(1024 * 1024),
        Compression::None,
        Some(options),
    );
    let logger = LOGGER.get_or_init(|| Logger {
        output: Mutex::new(output),
    });
    log::set_logger(logger).map_err(|_| "proxy logger already initialized")?;
    log::set_max_level(level);
    Ok(())
}

pub(crate) fn report_stderr(operation: &'static str, error: &(dyn core::error::Error + 'static)) {
    // If stderr also fails there is no remaining diagnostic sink; do not recurse.
    let _ = writeln!(
        std::io::stderr().lock(),
        "{operation} failed:{}",
        wizard_proxy::diagnostics::summary(error)
    );
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level() && metadata.target().starts_with("wizard_proxy")
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Match Wizard.log's default UTC timestamps and field layout.
        // One buffer per record prevents size-based rotation from splitting a line.
        let line = format!(
            "[{}][{}][{}] {}\n",
            chrono::Utc::now().format("%Y-%m-%d][%H:%M:%S"),
            record.target(),
            record.level(),
            record.args(),
        );
        if let Err(error) = output.write_all(line.as_bytes()) {
            report_stderr("proxy log write", &error);
            // Never recurse through the logger when its output is failing.
            let _ = std::io::stderr().lock().write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        let mut output = self
            .output
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Err(error) = output.flush() {
            report_stderr("proxy log flush", &error);
        }
    }
}
