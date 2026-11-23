use loco_rs::model::query::PaginationQuery;
use yorishiro::dtos::common::PageParams;
use yorishiro::models::pagination::ListParams;

#[test]
fn direct_list_parameters_preserve_defaults_and_clamp_signed_boundaries() {
    let defaults = ListParams::default();
    assert_eq!((defaults.limit(), defaults.offset()), (50, 0));
    for (limit, offset, expected) in [
        (None, None, (50, 0)),
        (Some(i64::MIN), Some(i64::MIN), (1, 0)),
        (Some(0), Some(-1), (1, 0)),
        (Some(200), Some(7), (200, 7)),
        (
            Some(i64::MAX),
            Some(i64::MAX),
            (200, i64::MAX.cast_unsigned()),
        ),
    ] {
        let params = ListParams::new(limit, offset);
        assert_eq!((params.limit(), params.offset()), expected);
    }
}

#[test]
fn extreme_pages_are_rejected_without_wrapping_to_the_first_page() {
    for (page, page_size) in [(u64::MAX, 50), (184_467_440_737_095_517, 100)] {
        let result = ListParams::try_from(PageParams(PaginationQuery { page, page_size }));
        assert!(matches!(
            result,
            Err(yorishiro::YorishiroError::ValidationFailed { .. })
        ));
    }
}

#[test]
fn ordinary_pagination_and_existing_clamps_are_preserved() {
    for (page, page_size, limit, offset) in [(3, 7, 7, 14), (0, 0, 1, 0), (2, u64::MAX, 200, 200)] {
        let result = ListParams::try_from(PageParams(PaginationQuery { page, page_size }))
            .expect("valid page");
        assert_eq!((result.limit(), result.offset()), (limit, offset));
    }
}
