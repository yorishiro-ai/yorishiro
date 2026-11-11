//! Application logging policy, installed through Loco's logger hook.
//!
//! Logger settings and filtering come from Loco's configuration.
//! `settings.log_output` selects stdout (default) or the systemd journal; `logger.file_appender` is rejected at config load.
//! The journal stamps its own time, so only the stdout layer uses the configured timezone.
//! Events use tracing with stable messages and structured fields, including `error` for failures.
//! CLI results remain on stdout; operational diagnostics belong in tracing events.
//! Error response rendering and internal-error reporting live in `crate::error`.

use std::fmt;

use tracing_subscriber::fmt::{format::Writer, time::FormatTime};

use crate::data::settings::LogOutput;

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

    let config = &ctx.config.logger;
    if config.enable {
        let filter =
            loco_rs::logger::init_env_filter::<H>(config.override_filter.as_ref(), &config.level);
        let output = crate::data::settings::configured(ctx)?.log_output;
        let journal = (output == LogOutput::Journald)
            .then(tracing_journald::layer)
            .and_then(Result::ok)
            .map(|layer| {
                Box::new(layer)
                    as Box<
                        dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Sync + Send,
                    >
            });
        // Without a reachable journal socket the process still logs, to stdout.
        let sink = journal.unwrap_or_else(|| {
            layer(
                &config.format,
                std::io::stdout,
                crate::time::configured(ctx),
            )
        });
        tracing_subscriber::registry()
            .with(sink)
            .with(filter)
            .try_init()
            .map_err(|error| loco_rs::Error::Message(error.to_string()))?;
    }
    Ok(true)
}

fn layer<W>(
    format: &loco_rs::logger::Format,
    writer: W,
    timezone: chrono_tz::Tz,
) -> Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Sync + Send>
where
    W: for<'writer> tracing_subscriber::fmt::MakeWriter<'writer> + Sync + Send + 'static,
{
    use tracing_subscriber::Layer;
    let base = tracing_subscriber::fmt::layer()
        .with_timer(ConfiguredTimer(timezone))
        .with_ansi(true)
        .with_writer(writer);
    match format {
        loco_rs::logger::Format::Compact => base.compact().boxed(),
        loco_rs::logger::Format::Pretty => base.pretty().boxed(),
        loco_rs::logger::Format::Json => base.json().boxed(),
    }
}
