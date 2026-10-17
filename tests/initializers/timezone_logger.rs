#[test]
fn configured_timer_renders_dst_offsets_and_json_safe_timestamp() {
    use tracing_subscriber::fmt::{format::Writer, time::FormatTime};

    let mut tokyo = String::new();
    yorishiro::initializers::timezone_logger::ConfiguredTimer::new(
        yorishiro::time::parse_timezone("Asia/Tokyo").unwrap(),
    )
    .format_time(&mut Writer::new(&mut tokyo))
    .unwrap();
    assert!(tokyo.ends_with(" "));
    assert!(tokyo.contains("+09:00"));

    let mut new_york = String::new();
    yorishiro::initializers::timezone_logger::ConfiguredTimer::new(
        yorishiro::time::parse_timezone("America/New_York").unwrap(),
    )
    .format_time(&mut Writer::new(&mut new_york))
    .unwrap();
    assert!(new_york.contains("-04:00") || new_york.contains("-05:00"));
    assert!(new_york.ends_with(" "));
    let json_timestamp = new_york.trim_end();
    assert!(!json_timestamp.ends_with(' '));
    assert!(json_timestamp.contains('T'));
    assert!(json_timestamp.contains('-'));
}
