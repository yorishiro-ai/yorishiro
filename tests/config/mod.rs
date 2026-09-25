mod init;
mod load;
mod overrides;
mod validate;

pub(crate) fn minimal_config(uri: &str) -> String {
    format!(
        "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: {uri}\nworkers:\n  mode: ForegroundBlocking\n"
    )
}

pub(crate) use crate::{CurrentDirGuard, EnvGuard};
