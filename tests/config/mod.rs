mod init;
mod load;
mod overrides;
mod validate;

use std::env;

pub(crate) fn minimal_config(uri: &str) -> String {
    format!(
        "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: {uri}\nworkers:\n  mode: ForegroundBlocking\n"
    )
}

pub(crate) struct EnvGuard {
    values: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    pub(crate) fn capture(variables: &[&'static str]) -> Self {
        Self {
            values: variables
                .iter()
                .map(|variable| (*variable, env::var_os(variable)))
                .collect(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            for (variable, original) in &self.values {
                match original {
                    Some(value) => env::set_var(variable, value),
                    None => env::remove_var(variable),
                }
            }
        }
    }
}

pub(crate) struct CurrentDirGuard {
    original: std::path::PathBuf,
}

impl CurrentDirGuard {
    pub(crate) fn enter(path: &std::path::Path) -> Self {
        let original = env::current_dir().unwrap();
        env::set_current_dir(path).unwrap();
        Self { original }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        env::set_current_dir(&self.original).unwrap();
    }
}
