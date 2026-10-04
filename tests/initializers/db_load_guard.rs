use std::time::{Duration, Instant};
use yorishiro::initializers::db_load_guard::*;
use yorishiro::models::system_maintenance::{AUTO_REASON, MaintenanceMode, MaintenanceState};

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

#[test]
fn measurement_error_resets_the_sustain_window_through_run_step() {
    let sustain = Duration::from_secs(30);
    let start = Instant::now();
    let mut history = LoadHistory::default();
    let current = state(MaintenanceMode::Off, None);
    assert!(matches!(
        run_step(
            &mut history,
            Ok(10),
            Ok(current.clone()),
            10,
            sustain,
            start
        ),
        StepOutcome::Continue
    ));
    assert!(matches!(
        run_step(&mut history, Err(()), Err(()), 10, sustain, start + sustain),
        StepOutcome::Continue
    ));
    assert!(matches!(
        run_step(
            &mut history,
            Ok(10),
            Ok(current.clone()),
            10,
            sustain,
            start + sustain + sustain - Duration::from_secs(1)
        ),
        StepOutcome::Continue
    ));
    assert!(matches!(
        run_step(
            &mut history,
            Ok(10),
            Ok(current),
            10,
            sustain,
            start + sustain + sustain + sustain
        ),
        StepOutcome::Transition {
            mode: MaintenanceMode::ReadOnly,
            ..
        }
    ));
}

#[test]
fn maintenance_state_error_resets_the_sustain_window_through_run_step() {
    let sustain = Duration::from_secs(30);
    let start = Instant::now();
    let mut history = LoadHistory::default();
    let current = state(MaintenanceMode::Off, None);
    assert!(matches!(
        run_step(
            &mut history,
            Ok(10),
            Ok(current.clone()),
            10,
            sustain,
            start
        ),
        StepOutcome::Continue
    ));
    assert!(matches!(
        run_step(&mut history, Ok(10), Err(()), 10, sustain, start + sustain),
        StepOutcome::Continue
    ));
    assert!(matches!(
        run_step(
            &mut history,
            Ok(10),
            Ok(current.clone()),
            10,
            sustain,
            start + sustain + sustain - Duration::from_secs(1)
        ),
        StepOutcome::Continue
    ));
    assert!(matches!(
        run_step(
            &mut history,
            Ok(10),
            Ok(current),
            10,
            sustain,
            start + sustain + sustain + sustain
        ),
        StepOutcome::Transition {
            mode: MaintenanceMode::ReadOnly,
            ..
        }
    ));
}
