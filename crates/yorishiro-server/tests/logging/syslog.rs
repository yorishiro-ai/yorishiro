#![cfg(unix)]

use std::io;
use std::os::unix::net::UnixDatagram;
use std::sync::Arc;

use crate::logging::syslog::{SyslogMakeWriter, severity_for_level};

#[test]
fn syslog_writer_sends_one_datagram_per_dropped_writer_with_the_right_pri() {
    let (client, server) = UnixDatagram::pair().unwrap();
    let make_writer = SyslogMakeWriter {
        socket: Arc::new(client),
    };

    {
        let mut writer = make_writer.writer_for_severity(6);
        // Two separate `write` calls (as tracing-subscriber issues for a formatted line plus its trailing newline) must still coalesce into a single datagram.
        io::Write::write_all(&mut writer, b"{\"message\":\"hello\"}").unwrap();
        io::Write::write_all(&mut writer, b"\n").unwrap();
    } // dropped here, which flushes

    let mut buf = [0u8; 256];
    let n = server.recv(&mut buf).unwrap();
    let received = std::str::from_utf8(&buf[..n]).unwrap();

    // facility (user, 1) * 8 + severity (informational, 6) = 14
    assert_eq!(received, "<14>yorishiro-server: {\"message\":\"hello\"}\n");
}

#[test]
fn severity_for_level_matches_rfc_5424() {
    assert_eq!(severity_for_level(tracing::Level::ERROR), 3);
    assert_eq!(severity_for_level(tracing::Level::WARN), 4);
    assert_eq!(severity_for_level(tracing::Level::INFO), 6);
    assert_eq!(severity_for_level(tracing::Level::DEBUG), 7);
    assert_eq!(severity_for_level(tracing::Level::TRACE), 7);
}

#[test]
fn writer_for_severity_frames_the_pri_correctly_for_an_error_level() {
    let (client, server) = UnixDatagram::pair().unwrap();
    let make_writer = SyslogMakeWriter {
        socket: Arc::new(client),
    };

    {
        let mut writer = make_writer.writer_for_severity(severity_for_level(tracing::Level::ERROR));
        io::Write::write_all(&mut writer, b"boom").unwrap();
    }

    let mut buf = [0u8; 256];
    let n = server.recv(&mut buf).unwrap();
    let received = std::str::from_utf8(&buf[..n]).unwrap();

    // facility (user, 1) * 8 + severity (error, 3) = 11
    assert_eq!(received, "<11>yorishiro-server: boom");
}

#[test]
fn flushing_an_empty_buffer_sends_nothing() {
    let (client, server) = UnixDatagram::pair().unwrap();
    server.set_nonblocking(true).unwrap();
    let make_writer = SyslogMakeWriter {
        socket: Arc::new(client),
    };

    drop(make_writer.writer_for_severity(6));

    let mut buf = [0u8; 16];
    assert!(
        server.recv(&mut buf).is_err(),
        "expected no datagram to arrive"
    );
}
