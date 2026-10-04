mod vector_blob_tests {
    use yorishiro::db::sqlite_vec_blob;

    #[test]
    fn blob_is_little_endian_f32_in_order() {
        assert_eq!(
            sqlite_vec_blob(&[1.0, -2.0]),
            [0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0xc0]
        );
        assert!(sqlite_vec_blob(&[]).is_empty());
    }
}
