use chrono::{Offset, TimeZone, Utc};
use yorishiro::time::{in_timezone, parse_timezone};

#[test]
fn timezone_rendering_preserves_epoch_and_applies_dst_offset() {
    let tokyo = parse_timezone("Asia/Tokyo").unwrap();
    let utc = Utc.with_ymd_and_hms(2026, 10, 16, 0, 0, 0).unwrap();
    let rendered = in_timezone(utc, tokyo);
    assert_eq!(rendered.timestamp(), utc.timestamp());
    assert_eq!(rendered.offset().fix().local_minus_utc(), 9 * 60 * 60);

    let new_york = parse_timezone("America/New_York").unwrap();
    let winter = Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap();
    let summer = Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap();
    assert_eq!(
        in_timezone(winter, new_york)
            .offset()
            .fix()
            .local_minus_utc(),
        -5 * 60 * 60
    );
    assert_eq!(
        in_timezone(summer, new_york)
            .offset()
            .fix()
            .local_minus_utc(),
        -4 * 60 * 60
    );
}

#[test]
fn configured_zone_restart_cycle_keeps_each_instant_and_uses_transition_offsets() {
    let instants = [
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 7, 15, 12, 0, 0).unwrap(),
    ];
    let zones = ["UTC", "Asia/Tokyo", "America/New_York", "UTC"];

    for zone in zones {
        let timezone = parse_timezone(zone).unwrap();
        for instant in instants {
            let rendered = in_timezone(instant, timezone);
            assert_eq!(
                rendered.timestamp_nanos_opt(),
                instant.timestamp_nanos_opt()
            );
        }
    }
    let new_york = parse_timezone("America/New_York").unwrap();
    assert_eq!(
        in_timezone(instants[0], new_york)
            .offset()
            .fix()
            .local_minus_utc(),
        -5 * 60 * 60
    );
    assert_eq!(
        in_timezone(instants[1], new_york)
            .offset()
            .fix()
            .local_minus_utc(),
        -4 * 60 * 60
    );
}

#[test]
fn configured_zone_parser_rejects_non_iana_names() {
    assert!(parse_timezone("Not/A_Zone").is_err());
    assert!(parse_timezone("Asia/Tokyo").is_ok());
}
