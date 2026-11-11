use std::time::{Duration, Instant};
use yorishiro::initializers::db_load_guard::*;
use yorishiro::models::system_maintenance::{AUTO_REASON, MaintenanceMode, MaintenanceState};

#[tokio::test]
async fn registered_diagnostic_task_queries_real_database_activity() {
    use loco_rs::{
        app::Hooks,
        task::{Tasks, Vars},
    };
    crate::requests::boot_request_isolated::<yorishiro::App, _, _>(
        |_request, mut ctx| async move {
            ctx.config.settings.as_mut().unwrap()["db_load_guard"]["threshold"] = 1.into();
            let before = yorishiro::models::system_maintenance::get(&ctx.db)
                .await
                .unwrap();
            let mut tasks = Tasks::default();
            yorishiro::App::register_tasks(&mut tasks);
            tasks
                .run(&ctx, "db_load_guard", &Vars::default())
                .await
                .unwrap();
            let after = yorishiro::models::system_maintenance::get(&ctx.db)
                .await
                .unwrap();
            assert_eq!(before.mode, after.mode);
            assert_eq!(before.reason, after.reason);
        },
    )
    .await;
}

fn state(mode: MaintenanceMode, reason: Option<&str>) -> MaintenanceState {
    MaintenanceState {
        mode,
        retry_after: 300,
        reason: reason.map(str::to_owned),
    }
}

#[test]
fn drops_to_read_only_only_after_sustained_load() {
    let sustain = Duration::from_secs(30);
    assert_eq!(
        decide(
            &state(MaintenanceMode::Off, None),
            Duration::from_secs(5),
            Duration::ZERO,
            sustain
        ),
        None
    );
    assert_eq!(
        decide(
            &state(MaintenanceMode::Off, None),
            sustain,
            Duration::ZERO,
            sustain
        ),
        Some(MaintenanceMode::ReadOnly)
    );
}

#[test]
fn lifts_only_guard_owned_read_only() {
    let sustain = Duration::from_secs(30);
    assert_eq!(
        decide(
            &state(MaintenanceMode::ReadOnly, Some(AUTO_REASON)),
            Duration::ZERO,
            sustain,
            sustain
        ),
        Some(MaintenanceMode::Off)
    );
    assert_eq!(
        decide(
            &state(MaintenanceMode::ReadOnly, Some("restore")),
            Duration::ZERO,
            sustain,
            sustain
        ),
        None
    );
    assert_eq!(
        decide(
            &state(MaintenanceMode::FullLock, Some(AUTO_REASON)),
            sustain,
            sustain,
            sustain
        ),
        None
    );
}

#[test]
fn first_sample_does_not_count_as_poll_time() {
    let sustain = Duration::from_secs(30);
    let start = Instant::now();
    let mut history = LoadHistory::default();
    let (busy_for, quiet_for) = history.observe(true, start);
    assert_eq!((busy_for, quiet_for), (Duration::ZERO, Duration::ZERO));
    let (busy_for, quiet_for) = history.observe(true, start + Duration::from_secs(29));
    assert_eq!(quiet_for, Duration::ZERO);
    assert_eq!(
        decide(
            &state(MaintenanceMode::Off, None),
            busy_for,
            quiet_for,
            sustain
        ),
        None
    );
    let (busy_for, quiet_for) = history.observe(true, start + sustain);
    assert_eq!(quiet_for, Duration::ZERO);
    assert_eq!(
        decide(
            &state(MaintenanceMode::Off, None),
            busy_for,
            quiet_for,
            sustain
        ),
        Some(MaintenanceMode::ReadOnly)
    );
}

/// A failed reading (of the load, or of the maintenance state) forgets the time already spent over the threshold.
fn failed_reading_resets_the_sustain_window(load_fails: bool) {
    let sustain = Duration::from_secs(30);
    let start = Instant::now();
    let mut history = LoadHistory::default();
    let current = state(MaintenanceMode::Off, None);
    let over = |history: &mut LoadHistory, at| {
        run_step(history, Ok(10), Ok(current.clone()), 10, sustain, at)
    };
    assert!(matches!(over(&mut history, start), StepOutcome::Continue));
    let load = if load_fails { Err(()) } else { Ok(10) };
    assert!(matches!(
        run_step(&mut history, load, Err(()), 10, sustain, start + sustain),
        StepOutcome::Continue
    ));
    assert!(matches!(
        over(
            &mut history,
            start + sustain + sustain - Duration::from_secs(1)
        ),
        StepOutcome::Continue
    ));
    assert!(matches!(
        over(&mut history, start + sustain + sustain + sustain),
        StepOutcome::Transition {
            mode: MaintenanceMode::ReadOnly,
            ..
        }
    ));
}

#[test]
fn measurement_error_resets_the_sustain_window_through_run_step() {
    failed_reading_resets_the_sustain_window(true);
}

#[test]
fn maintenance_state_error_resets_the_sustain_window_through_run_step() {
    failed_reading_resets_the_sustain_window(false);
}
