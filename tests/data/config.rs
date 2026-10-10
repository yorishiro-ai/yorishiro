use yorishiro::data::config::validate_queue_policy;

fn config(database: &str, queue: &str) -> loco_rs::config::Config {
    serde_yaml::from_str(&format!(
        "logger: {{ enable: true, pretty_backtrace: false, level: info, format: compact }}\nserver: {{ port: 5150, binding: localhost, host: http://localhost }}\ndatabase: {{ uri: '{database}', enable_logging: false, connect_timeout: 500, idle_timeout: 500, min_connections: 1, max_connections: 2, auto_migrate: false }}\nqueue: {{ kind: Sqlite, uri: '{queue}', num_workers: 1 }}\nworkers: {{ mode: BackgroundQueue }}\n"
    ))
    .unwrap()
}

#[test]
fn equivalent_sqlite_paths_are_rejected_but_memory_is_not() {
    let shared = config(
        "sqlite://app.sqlite3?mode=rwc",
        "sqlite://./app.sqlite3?mode=ro",
    );
    assert!(validate_queue_policy(&shared).is_err());
    let shared = config(
        "sqlite:/tmp/app.sqlite3?mode=rwc",
        "sqlite:///tmp/app.sqlite3",
    );
    assert!(validate_queue_policy(&shared).is_err());

    let memory = config("sqlite::memory:", "sqlite://?mode=memory&cache=shared");
    assert!(validate_queue_policy(&memory).is_ok());
}

fn redis_config(queues: &str) -> loco_rs::config::Config {
    serde_yaml::from_str(&format!(
        "logger: {{ enable: true, pretty_backtrace: false, level: info, format: compact }}\nserver: {{ port: 5150, binding: localhost, host: http://localhost }}\ndatabase: {{ uri: 'sqlite::memory:', enable_logging: false, connect_timeout: 500, idle_timeout: 500, min_connections: 1, max_connections: 2, auto_migrate: false }}\nqueue: {{ kind: Redis, uri: 'redis://localhost:6379', num_workers: 1{queues} }}\nworkers: {{ mode: BackgroundQueue }}\n"
    ))
    .unwrap()
}

fn served(config: &loco_rs::config::Config) -> Option<Vec<String>> {
    match config.queue.as_ref() {
        Some(loco_rs::config::QueueConfig::Redis(redis)) => redis.queues.clone(),
        _ => None,
    }
}

#[test]
fn a_redis_queue_serves_every_worker_queue_once_and_keeps_its_own() {
    let wanted = vec!["a".to_owned(), "b".to_owned()];

    let mut unset = redis_config("");
    yorishiro::data::config::serve_worker_queues(&mut unset, &wanted);
    assert_eq!(served(&unset), Some(wanted.clone()));

    let mut partial = redis_config(", queues: [custom, b]");
    yorishiro::data::config::serve_worker_queues(&mut partial, &wanted);
    yorishiro::data::config::serve_worker_queues(&mut partial, &wanted);
    assert_eq!(
        served(&partial),
        Some(vec!["custom".to_owned(), "b".to_owned(), "a".to_owned()])
    );
}

#[test]
fn a_sql_queue_is_left_alone() {
    let mut sqlite = config(
        "sqlite://app.sqlite3?mode=rwc",
        "sqlite://queue.sqlite3?mode=rwc",
    );
    let before = format!("{:?}", sqlite.queue);
    yorishiro::data::config::serve_worker_queues(&mut sqlite, &["a".to_owned()]);
    assert_eq!(format!("{:?}", sqlite.queue), before);
}

#[test]
fn cross_sql_queue_topologies_are_supported() {
    let postgres_database = config(
        "postgres://localhost:5432/app",
        "sqlite://queue.sqlite3?mode=rwc",
    );
    assert!(validate_queue_policy(&postgres_database).is_ok());

    let sqlite_database = serde_yaml::from_str::<loco_rs::config::Config>(
        "logger: { enable: true, pretty_backtrace: false, level: info, format: compact }\nserver: { port: 5150, binding: localhost, host: http://localhost }\ndatabase: { uri: 'sqlite://app.sqlite3?mode=rwc', enable_logging: false, connect_timeout: 500, idle_timeout: 500, min_connections: 1, max_connections: 2, auto_migrate: false }\nqueue: { kind: Postgres, uri: 'postgres://localhost:5432/queue', num_workers: 1 }\nworkers: { mode: BackgroundQueue }\n",
    )
    .unwrap();
    assert!(validate_queue_policy(&sqlite_database).is_ok());
}

fn topology(database: &str, max_connections: u32, queue: &str) -> loco_rs::config::Config {
    serde_yaml::from_str(&format!(
        "logger: {{ enable: true, pretty_backtrace: false, level: info, format: compact }}\nserver: {{ port: 5150, binding: localhost, host: http://localhost }}\ndatabase: {{ uri: '{database}', enable_logging: false, connect_timeout: 500, idle_timeout: 500, min_connections: 1, max_connections: {max_connections}, auto_migrate: false }}\nqueue: {queue}\nworkers: {{ mode: BackgroundQueue }}\n"
    ))
    .unwrap()
}

#[test]
fn only_topologies_that_cannot_use_the_setting_are_flagged() {
    use yorishiro::data::config::{SQLITE_POOL_ADVISORY_LIMIT, topology_advisories};

    let sqlite_queue = |workers: u32| {
        format!("{{ kind: Sqlite, uri: 'sqlite://q.sqlite3?mode=rwc', num_workers: {workers} }}")
    };
    let valkey = |workers: u32| {
        format!("{{ kind: Redis, uri: 'redis://localhost:6379', num_workers: {workers} }}")
    };
    let postgres = |workers: u32| {
        format!("{{ kind: Postgres, uri: 'postgres://localhost/q', num_workers: {workers} }}")
    };
    let sqlite = "sqlite://app.sqlite3?mode=rwc";
    let pg = "postgres://localhost/app";

    assert!(topology_advisories(&topology(sqlite, 10, &sqlite_queue(1))).is_empty());
    assert_eq!(
        topology_advisories(&topology(sqlite, 10, &sqlite_queue(4))).len(),
        1
    );
    assert_eq!(
        topology_advisories(&topology(
            sqlite,
            SQLITE_POOL_ADVISORY_LIMIT + 1,
            &sqlite_queue(1)
        ))
        .len(),
        1
    );
    assert_eq!(
        topology_advisories(&topology(
            sqlite,
            SQLITE_POOL_ADVISORY_LIMIT + 1,
            &sqlite_queue(4)
        ))
        .len(),
        2
    );
    // A networked queue over a SQLite file: workers elsewhere cannot reach the file, and its named locks are no-ops.
    for queue in [valkey(8), postgres(8)] {
        let advisories = topology_advisories(&topology(sqlite, SQLITE_POOL_ADVISORY_LIMIT, &queue));
        assert_eq!(advisories.len(), 1);
        assert!(advisories[0].contains("SQLite"), "{advisories:?}");
    }
    assert!(topology_advisories(&topology(pg, 100, &valkey(8))).is_empty());
    assert!(topology_advisories(&topology(pg, 100, &postgres(8))).is_empty());
    assert_eq!(
        topology_advisories(&topology(pg, 100, &sqlite_queue(2))).len(),
        1
    );
}

fn with_queue(num_workers: u32) -> loco_rs::config::Config {
    serde_yaml::from_str(&format!(
        "logger: {{ enable: true, pretty_backtrace: false, level: info, format: compact }}\nserver: {{ port: 5150, binding: localhost, host: http://localhost }}\ndatabase: {{ uri: 'postgres://localhost/app', enable_logging: false, connect_timeout: 500, idle_timeout: 500, min_connections: 1, max_connections: 2, auto_migrate: false }}\nqueue: {{ kind: Redis, uri: 'redis://localhost:6379', num_workers: {num_workers} }}\nworkers: {{ mode: BackgroundQueue }}\n"
    ))
    .unwrap()
}

#[test]
fn a_queue_with_no_workers_is_refused() {
    assert!(validate_queue_policy(&with_queue(1)).is_ok());
    let error = validate_queue_policy(&with_queue(0))
        .unwrap_err()
        .to_string();
    assert!(error.contains("num_workers"), "{error}");
}

#[test]
fn only_the_built_in_queue_mode_is_accepted() {
    let mut inline = with_queue(1);
    inline.workers.mode = loco_rs::config::WorkerMode::ForegroundBlocking;
    let error = validate_queue_policy(&inline).unwrap_err().to_string();
    assert!(error.contains("worker mode"), "{error}");

    let mut no_queue = with_queue(1);
    no_queue.queue = None;
    let error = validate_queue_policy(&no_queue).unwrap_err().to_string();
    assert!(error.contains("queue provider"), "{error}");
}
