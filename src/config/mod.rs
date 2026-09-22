mod init;
mod load;
mod overrides;
mod validate;

pub use init::init;
pub use load::load;

pub const CANONICAL_CONFIG_FILE: &str = "yorishiro.yaml";
const CONFIG_PATH_ENV: &str = "YORISHIRO_CONFIG_PATH";

pub const CONFIG_SKELETON: &str = r#"# Yorishiro configuration.
#
# This is plain YAML. Yorishiro does not evaluate Tera templates or get_env expressions here.
# Environment variables listed in the comments below explicitly override these values.

# Logging. LOG_LEVEL overrides logger.level. RUST_LOG has final control over filtering.
logger:
  enable: true
  pretty_backtrace: false
  level: info
  format: compact

# HTTP server. PORT, BINDING, and HOST override the corresponding values.
server:
  port: 5150
  binding: 0.0.0.0
  host: http://localhost
  middlewares:
    request_id:
      enable: true
    logger:
      enable: true

# Database. DATABASE_URL, DB_LOGGING, DB_CONNECT_TIMEOUT, DB_IDLE_TIMEOUT, DB_MIN_CONNECTIONS, DB_MAX_CONNECTIONS, and DB_AUTO_MIGRATE override these values.
# SQLite is for a single tenant. Use PostgreSQL for multi-tenant deployments.
database:
  # The CLI skeleton uses a writable path relative to the working directory.
  # Package and Docker installations replace this with /var/lib/yorishiro.
  uri: sqlite://yorishiro.sqlite3?mode=rwc
  enable_logging: false
  connect_timeout: 500
  idle_timeout: 500
  min_connections: 1
  max_connections: 100
  auto_migrate: true
  dangerously_truncate: false
  dangerously_recreate: false

# Background jobs. QUEUE_URL, YORISHIRO_QUEUE_WORKERS, and YORISHIRO_QUEUE_REAPER_AGE_MINUTES override fields in this block.
# YORISHIRO_QUEUE_KIND can switch between Sqlite, Postgres, and Redis.
workers:
  mode: BackgroundQueue
queue:
  kind: Sqlite
  uri: sqlite://yorishiro.sqlite3?mode=rwc
  dangerously_flush: false
  num_workers: 2
  reaper:
    age_minutes: 30
    interval_seconds: 60

# Add scheduled tasks here when this process runs `yorishiro scheduler`.
# scheduler:
#   output: stdout
#   jobs: {}

# Configure mail only when the deployment sends email. MAILER_HOST, MAILER_PORT, MAILER_USER, and MAILER_PASSWORD override a configured SMTP block.
# mailer:
#   smtp:
#     enable: true
#     host: smtp.example.com
#     port: 587
#     secure: true
#     auth:
#       user: example
#       password: change-me

# Application and enterprise settings are intentionally environment-only.
# Worker startup tags use YORISHIRO_WORKER_TAGS (comma-separated):
# worker-class:tenant-private, worker-class:official, worker-class:shared, or infer-fill.
# Scheduler jobs may be declared in scheduler.jobs and run with `yorishiro scheduler`.
# Embedding variables include YORISHIRO_EMBEDDING_PROVIDER, YORISHIRO_EMBEDDING_MODEL,
# YORISHIRO_EMBEDDING_BASE_URL, and YORISHIRO_EMBEDDING_API_KEY.
# Limits and operations include YORISHIRO_MAX_TENANTS, YORISHIRO_LOAD_GUARD_*,
# and YORISHIRO_RATE_LIMIT_*.
# Enterprise variables (licence, OAuth/OIDC, Stripe, marketplace, and worker classes)
# are read directly by `ee/`; they are deliberately not duplicated as YAML fields.
"#;
