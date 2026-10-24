use loco_rs::model::query::PaginationQuery;
use yorishiro::dtos::common::PageParams;
use yorishiro::models::pagination::ListParams;

#[test]
fn extreme_pages_are_rejected_without_wrapping_to_the_first_page() {
    for (page, page_size) in [(u64::MAX, 50), (184467440737095517, 100)] {
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
