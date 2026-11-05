use yorishiro::app::cli_requires_timezone_gate;

#[test]
fn gates_worker_commands_after_global_environment_options() {
    for args in [
        vec!["app", "task", "sync"],
        vec!["app", "-e", "production", "task", "sync"],
        vec!["app", "--environment=production", "scheduler"],
        vec!["app", "task", "-e", "production", "sync"],
        vec!["app", "--verbose", "jobs", "purge"],
        vec!["app", "-eproduction", "jobs", "purge"],
    ] {
        assert!(cli_requires_timezone_gate(args));
    }
}

#[test]
fn does_not_gate_bootstrap_or_command_like_environment_values() {
    for args in [
        vec!["app", "db", "migrate"],
        vec!["app", "db", "entities"],
        vec!["app", "generate", "model"],
        vec!["app", "-e", "task", "routes"],
        vec!["app", "--environment", "scheduler", "routes"],
        vec!["app", "db", "migrate", "--environment", "production"],
    ] {
        assert!(!cli_requires_timezone_gate(args));
    }
}

#[test]
fn task_gate_does_not_treat_flags_as_commands() {
    for args in [
        vec!["app", "--help"],
        vec!["app", "db", "migrate", "-e", "production"],
    ] {
        assert!(!cli_requires_timezone_gate(args));
    }
}
