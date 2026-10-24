//! Application logging policy, installed through Loco's logger hook.
//!
//! Logger settings and filtering come from Loco's configuration.
//! Events use tracing with stable messages and structured fields, including `error` for failures.
//! CLI results remain on stdout; operational diagnostics belong in tracing events.
//! Error response rendering and internal-error reporting live in `crate::error`.

use std::fmt;

use tracing_subscriber::fmt::{format::Writer, time::FormatTime};

pub struct ConfiguredTimer(chrono_tz::Tz);

impl ConfiguredTimer {
    pub fn new(timezone: chrono_tz::Tz) -> Self {
        Self(timezone)
    }
}

impl FormatTime for ConfiguredTimer {
    fn format_time(&self, writer: &mut Writer<'_>) -> fmt::Result {
        let timestamp = crate::time::in_timezone(crate::time::now(), self.0).to_rfc3339();
        write!(writer, "{timestamp} ")
    }
}

pub(crate) fn init<H: loco_rs::app::Hooks>(
    ctx: &loco_rs::app::AppContext,
) -> loco_rs::Result<bool> {
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

    let timezone = crate::time::configured(ctx);
    let config = &ctx.config.logger;
    let mut layers: Vec<
        Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Sync + Send>,
    > = Vec::new();
    if config.enable {
        layers.push(layer(&config.format, std::io::stdout, timezone, true));
    }
    if let Some(file) = config.file_appender.as_ref().filter(|file| file.enable) {
        let rotation = match file.rotation {
            loco_rs::logger::Rotation::Minutely => tracing_appender::rolling::Rotation::MINUTELY,
            loco_rs::logger::Rotation::Hourly => tracing_appender::rolling::Rotation::HOURLY,
            loco_rs::logger::Rotation::Daily => tracing_appender::rolling::Rotation::DAILY,
            loco_rs::logger::Rotation::Never => tracing_appender::rolling::Rotation::NEVER,
        };
        let appender = tracing_appender::rolling::Builder::default()
            .rotation(rotation)
            .max_log_files(file.max_log_files)
            .filename_prefix(file.filename_prefix.as_deref().unwrap_or_default())
            .filename_suffix(file.filename_suffix.as_deref().unwrap_or_default())
            .build(file.dir.as_deref().unwrap_or("./logs"))
            .map_err(loco_rs::Error::msg)?;
        if file.non_blocking {
            let (writer, guard) = tracing_appender::non_blocking(appender);
            FILE_GUARD
                .set(guard)
                .map_err(|_| loco_rs::Error::Message("logger file guard already set".into()))?;
            layers.push(layer(&file.format, writer, timezone, false));
        } else {
            layers.push(layer(&file.format, appender, timezone, false));
        }
    }
    if !layers.is_empty() {
        let filter =
            loco_rs::logger::init_env_filter::<H>(config.override_filter.as_ref(), &config.level);
        tracing_subscriber::registry()
            .with(layers)
            .with(filter)
            .try_init()
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    }
    Ok(true)
}

static FILE_GUARD: std::sync::OnceLock<tracing_appender::non_blocking::WorkerGuard> =
    std::sync::OnceLock::new();

fn layer<W>(
    format: &loco_rs::logger::Format,
    writer: W,
    timezone: chrono_tz::Tz,
    ansi: bool,
) -> Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Sync + Send>
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Sync + Send + 'static,
{
    use tracing_subscriber::Layer;
    let base = tracing_subscriber::fmt::layer()
        .with_timer(ConfiguredTimer(timezone))
        .with_ansi(ansi)
        .with_writer(writer);
    match format {
        loco_rs::logger::Format::Compact => base.compact().boxed(),
        loco_rs::logger::Format::Pretty => base.pretty().boxed(),
        loco_rs::logger::Format::Json => base.json().boxed(),
    }
}
