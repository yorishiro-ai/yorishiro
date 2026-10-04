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
