//! Selects where log output (including the HTTP access log emitted by `TraceLayer`) goes,
//! controlled by `YORISHIRO_LOG_TARGET`:
//!
//! - `stdout` (default): JSON lines on stdout, for a container runtime's log driver.
//! - `single`: JSON lines appended to one file that's never rotated.
//! - `daily`: JSON lines appended to a file that rotates at midnight UTC.
//! - `syslog`: forwarded to the local syslog daemon over `/dev/log`, RFC 3164-framed (see the
//!   `syslog` submodule for the datagram framing). Unix-only: `/dev/log` is a Unix domain
//!   socket, so this target is rejected at startup on other platforms.
//!
//! `single`/`daily` write under `YORISHIRO_LOG_DIR` (default `.`) as `yorishiro.log`. `syslog`
//! connects to the socket at `YORISHIRO_SYSLOG_SOCKET` (default `/dev/log`).
pub mod syslog;

use anyhow::Result;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;

/// Owns whatever background resource the chosen log target needs to keep running (a non-blocking writer thread, for the file targets).
/// Dropping it would stop that thread, so the caller must hold it for the process's entire lifetime: binding it to `main`'s last local variable is enough.
pub enum LogGuard {
    None,
    // Never read; held only so its `Drop` (which stops the writer thread) fires at the end of `main` instead of immediately after `init` returns.
    NonBlocking(#[allow(dead_code)] WorkerGuard),
}

pub fn init() -> Result<LogGuard> {
    let target = std::env::var("YORISHIRO_LOG_TARGET").unwrap_or_else(|_| "stdout".into());
    let env_filter = EnvFilter::from_default_env();

    match target.as_str() {
        "stdout" => {
            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .json()
                .init();
            Ok(LogGuard::None)
        }
        "single" | "daily" => {
            let dir = std::env::var("YORISHIRO_LOG_DIR").unwrap_or_else(|_| ".".into());
            let rotation = if target == "daily" {
                Rotation::DAILY
            } else {
                Rotation::NEVER
            };
            let appender = RollingFileAppender::new(rotation, &dir, "yorishiro.log");
            let (writer, guard) = tracing_appender::non_blocking(appender);
            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .json()
                .with_writer(writer)
                .with_ansi(false)
                .init();
            Ok(LogGuard::NonBlocking(guard))
        }
        #[cfg(unix)]
        "syslog" => {
            use std::os::unix::net::UnixDatagram;
            use std::sync::Arc;

            use anyhow::Context;

            use syslog::SyslogMakeWriter;

            let socket_path =
                std::env::var("YORISHIRO_SYSLOG_SOCKET").unwrap_or_else(|_| "/dev/log".into());
            let socket = UnixDatagram::unbound().context("failed to create syslog socket")?;
            socket
                .connect(&socket_path)
                .with_context(|| format!("failed to connect to syslog socket at {socket_path}"))?;
            tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .json()
                .with_writer(SyslogMakeWriter {
                    socket: Arc::new(socket),
                })
                .with_ansi(false)
                .init();
            Ok(LogGuard::None)
        }
        #[cfg(not(unix))]
        "syslog" => {
            anyhow::bail!("the 'syslog' log target is only supported on unix platforms")
        }
        other => {
            anyhow::bail!(
                "unknown YORISHIRO_LOG_TARGET '{other}' (expected 'stdout', 'single', 'daily', or 'syslog')"
            )
        }
    }
}

#[cfg(test)]
#[path = "../../tests/logging/mod.rs"]
mod tests;
