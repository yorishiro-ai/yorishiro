//! RFC 3164 syslog writer for the `syslog` log target. `SyslogMakeWriter` hands
//! `tracing-subscriber` one `SyslogWriter` per event; each buffers the formatted line and
//! sends it as a single `/dev/log` datagram on drop, with a priority derived from the event's
//! level.
use std::io;
use std::os::unix::net::UnixDatagram;
use std::sync::Arc;

use tracing_subscriber::fmt::MakeWriter;

/// RFC 3164 facility code for "user-level messages", the conventional facility for
/// applications that aren't a system daemon.
const FACILITY_USER: u8 = 1;

#[derive(Clone)]
pub struct SyslogMakeWriter {
    pub socket: Arc<UnixDatagram>,
}

impl<'a> MakeWriter<'a> for SyslogMakeWriter {
    type Writer = SyslogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.writer_for_severity(6) // informational
    }

    fn make_writer_for(&'a self, meta: &tracing::Metadata<'_>) -> Self::Writer {
        self.writer_for_severity(severity_for_level(*meta.level()))
    }
}

/// Maps a tracing level to its RFC 5424 severity number.
///
/// `pub` (rather than private) solely so the crate-root `tests/` integration tests -- which
/// only see this crate's public API -- can exercise this mapping directly.
pub fn severity_for_level(level: tracing::Level) -> u8 {
    match level {
        tracing::Level::ERROR => 3,
        tracing::Level::WARN => 4,
        tracing::Level::INFO => 6,
        tracing::Level::DEBUG | tracing::Level::TRACE => 7,
    }
}

impl SyslogMakeWriter {
    /// `pub` (rather than private) solely so `tests/` can construct a writer at a specific
    /// severity directly, rather than only through the `MakeWriter` trait.
    pub fn writer_for_severity(&self, severity: u8) -> SyslogWriter {
        SyslogWriter {
            socket: self.socket.clone(),
            severity,
            buf: Vec::new(),
        }
    }
}

/// One instance is created per log event (via `make_writer_for`) and dropped right after
/// `tracing-subscriber` finishes formatting into it. Buffering until that drop, rather than
/// sending on every `write` call, guarantees the whole formatted line goes out as a single
/// syslog datagram instead of being split across several.
pub struct SyslogWriter {
    socket: Arc<UnixDatagram>,
    severity: u8,
    buf: Vec<u8>,
}

impl io::Write for SyslogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let pri = FACILITY_USER * 8 + self.severity;
        let mut datagram = format!("<{pri}>yorishiro-server: ").into_bytes();
        datagram.extend_from_slice(&self.buf);
        self.socket.send(&datagram)?;
        self.buf.clear();
        Ok(())
    }
}

impl Drop for SyslogWriter {
    fn drop(&mut self) {
        let _ = io::Write::flush(self);
    }
}
