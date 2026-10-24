use axum::http::StatusCode;
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
    if let Some(loco_rs::config::QueueConfig::Redis(redis)) = partial.queue.as_mut() {
        redis.dangerously_flush = true;
    }
    yorishiro::data::config::serve_worker_queues(&mut partial, &wanted);
    yorishiro::data::config::serve_worker_queues(&mut partial, &wanted);
    assert_eq!(
        served(&partial),
        Some(vec!["custom".to_owned(), "b".to_owned(), "a".to_owned()])
    );
    let queue = partial.queue.as_mut().unwrap();
    assert!(queue.dangerously_flush());
    queue.preserve_jobs();
    assert!(!queue.dangerously_flush());
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
    let queue = sqlite.queue.as_mut().unwrap();
    if let loco_rs::config::QueueConfig::Sqlite(sqlite) = queue {
        sqlite.dangerously_flush = true;
    }
    assert!(queue.dangerously_flush());
    queue.preserve_jobs();
    assert!(!queue.dangerously_flush());
}

#[test]
fn cross_sql_queue_topologies_fail_validation_with_actionable_errors() {
    let postgres_database = config(
        "postgres://localhost:5432/app",
        "sqlite://queue.sqlite3?mode=rwc",
    );

    let sqlite_database = serde_yaml::from_str::<loco_rs::config::Config>(
        "logger: { enable: true, pretty_backtrace: false, level: info, format: compact }\nserver: { port: 5150, binding: localhost, host: http://localhost }\ndatabase: { uri: 'sqlite://app.sqlite3?mode=rwc', enable_logging: false, connect_timeout: 500, idle_timeout: 500, min_connections: 1, max_connections: 2, auto_migrate: false }\nqueue: { kind: Postgres, uri: 'postgres://localhost:5432/queue', num_workers: 1 }\nworkers: { mode: BackgroundQueue }\n",
    )
    .unwrap();
    for config in [postgres_database, sqlite_database] {
        let loco_rs::Error::CustomError(status, detail) =
            validate_queue_policy(&config).expect_err("crossed SQL queues are forbidden")
        else {
            panic!("expected ValidationFailed");
        };
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let body = detail.errors.unwrap();
        assert_eq!(body["error"]["code"], "validation_failed");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.contains("not allowed"), "{body}");
        assert!(message.contains("queue"), "{body}");
        assert!(
            body["error"]["hint"]
                .as_str()
                .unwrap()
                .contains("Redis-compatible"),
            "{body}"
        );
    }
}

fn topology(database: &str, max_connections: u32, queue: &str) -> loco_rs::config::Config {
    serde_yaml::from_str(&format!(
        "logger: {{ enable: true, pretty_backtrace: false, level: info, format: compact }}\nserver: {{ port: 5150, binding: localhost, host: http://localhost }}\ndatabase: {{ uri: '{database}', enable_logging: false, connect_timeout: 500, idle_timeout: 500, min_connections: 1, max_connections: {max_connections}, auto_migrate: false }}\nqueue: {queue}\nworkers: {{ mode: BackgroundQueue }}\n"
    ))
    .unwrap()
}

#[test]
fn topology_advisories_cover_contention_and_support_boundaries() {
    use yorishiro::data::config::{SQLITE_POOL_ADVISORY_LIMIT, topology_advisories};

    let sqlite_queue = |workers: u32| {
        format!("{{ kind: Sqlite, uri: 'sqlite://q.sqlite3?mode=rwc', num_workers: {workers} }}")
    };
    let redis = |workers: u32| {
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
    let advisories = topology_advisories(&topology(sqlite, SQLITE_POOL_ADVISORY_LIMIT, &redis(8)));
    assert_eq!(advisories.len(), 1);
    assert!(advisories[0].contains("supported"), "{advisories:?}");
    assert!(advisories[0].contains("database's host"), "{advisories:?}");
    assert!(topology_advisories(&topology(pg, 100, &redis(8))).is_empty());
    assert!(topology_advisories(&topology(pg, 100, &postgres(8))).is_empty());
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
