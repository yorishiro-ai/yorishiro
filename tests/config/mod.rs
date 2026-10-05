mod load;
mod server_only_embedding;
mod validate;

pub(crate) fn minimal_config(uri: &str) -> String {
    format!(
        "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: sqlite://queue.sqlite3?mode=rwc\n  num_workers: 1\nworkers:\n  mode: BackgroundQueue\n"
    )
}

pub(crate) fn minimal_config_with_local_embedding(uri: &str) -> String {
    format!(
        "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: sqlite://queue.sqlite3?mode=rwc\n  num_workers: 1\nworkers:\n  mode: BackgroundQueue\nsettings:\n  max_tenants: 1\n  embedding:\n    provider: local\n    dimensions: 768\n    local_model: multilingual-e5-base\n    api_key: \"\"\n    send_dimensions_param: false\n    local_max_sequence_length: 512\n  rate_limit:\n    auth_max_requests: 10\n    auth_window_seconds: 60\n    search_tokens_per_minute: 100000\n  db_load_guard:\n    threshold: 0\n    sustain_seconds: 30\n    poll_seconds: 5\n"
    )
}

pub(crate) fn minimal_config_with_openai_embedding(uri: &str) -> String {
    format!(
        "logger:\n  enable: true\n  pretty_backtrace: false\n  level: info\n  format: compact\nserver:\n  port: 5150\n  binding: localhost\n  host: http://localhost\ndatabase:\n  uri: {uri}\n  enable_logging: false\n  connect_timeout: 500\n  idle_timeout: 500\n  min_connections: 1\n  max_connections: 10\n  auto_migrate: false\nqueue:\n  kind: Sqlite\n  uri: sqlite://queue.sqlite3?mode=rwc\n  num_workers: 1\nworkers:\n  mode: BackgroundQueue\nsettings:\n  max_tenants: 1\n  embedding:\n    provider: openai\n    dimensions: 768\n    base_url: http://localhost:8080\n    model: test-model\n    api_key: \"\"\n    send_dimensions_param: false\n    local_model: multilingual-e5-base\n    local_max_sequence_length: 512\n  rate_limit:\n    auth_max_requests: 10\n    auth_window_seconds: 60\n    search_tokens_per_minute: 100000\n  db_load_guard:\n    threshold: 0\n    sustain_seconds: 30\n    poll_seconds: 5\n"
    )
}

pub(crate) use crate::EnvGuard;
