//! Shared wall-clock and timezone policy for application, database, and queue boundaries.
//!
//! Domain durations and monotonic deadlines remain owned by their callers.
//! Database-generated timestamps keep the database clock and compare as instants.

use chrono::{DateTime, Utc};
use chrono_tz::Tz;

pub type Timezone = Tz;

/// Returns the current absolute instant for application-owned wall-clock operations.
#[must_use]
pub(crate) fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Adapts an instant to SeaORM's fixed-offset timestamp type without changing its epoch.
#[must_use]
pub(crate) fn database_instant(instant: DateTime<Utc>) -> DateTime<chrono::FixedOffset> {
    instant.into()
}

/// Formats an instant in SQLite's offset-bearing timestamp text shape for the configured zone.
#[must_use]
pub(crate) fn sqlite_timestamp<T: chrono::TimeZone>(instant: DateTime<T>, timezone: Tz) -> String {
    instant
        .with_timezone(&timezone)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        .to_string()
}

/// Parses a configured IANA timezone name.
///
/// # Errors
/// Returns an error if the name is not recognized as an IANA timezone.
pub fn parse_timezone(name: &str) -> Result<Tz, chrono_tz::ParseError> {
    name.parse()
}

/// Renders an instant in the configured IANA timezone without changing the instant.
pub fn in_timezone<T: chrono::TimeZone>(instant: DateTime<T>, timezone: Tz) -> DateTime<Tz> {
    instant.with_timezone(&timezone)
}

/// Renders an instant using the validated zone configured for this application context.
#[must_use]
pub(crate) fn render<T: chrono::TimeZone>(
    ctx: &loco_rs::app::AppContext,
    instant: DateTime<T>,
) -> DateTime<Tz> {
    in_timezone(instant, configured(ctx))
}

/// Returns the configured zone from validated application settings.
#[must_use]
pub(crate) fn configured(ctx: &loco_rs::app::AppContext) -> Tz {
    ctx.config
        .settings::<crate::data::settings::Settings>()
        .expect("timezone settings validated during config load")
        .timezone
        .parse()
        .expect("timezone validated during config load")
}
