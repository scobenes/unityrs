use unityrs_batch::winners::layout::{LIMIT, Layout, ZIP16, ZIP32};

#[test]
fn stored_layout_and_thresholds() {
    let mut layout = Layout::default();
    let entry = layout.member(5, 7).unwrap();
    assert_eq!(
        (entry.offset, entry.local_extra, entry.central_extra),
        (0, 0, 0)
    );
    let end = layout.finish().unwrap();
    assert_eq!(
        (end.central_offset, end.central_size, end.size, end.zip64),
        (42, 51, 115, false)
    );
    for size in [ZIP32 - 1, ZIP32, ZIP32 + 1] {
        let mut layout = Layout::default();
        let entry = layout.member(1, size).unwrap();
        assert_eq!(entry.local_extra, if size < ZIP32 { 0 } else { 20 });
        assert_eq!(entry.central_extra, entry.local_extra);
        let entry = layout.member(1, 1).unwrap();
        assert_eq!(entry.local_extra, 0);
        assert_eq!(entry.central_extra, 12);
        assert!(layout.finish().unwrap().zip64);
    }
}

#[test]
fn count_boundary_and_publication_limit() {
    let mut layout = Layout::default();
    for _ in 0..ZIP16 - 1 {
        layout.member(1, 0).unwrap();
    }
    assert!(!layout.finish().unwrap().zip64);
    layout.member(1, 0).unwrap();
    assert!(layout.finish().unwrap().zip64);
    let mut layout = Layout::default();
    layout.member(1, LIMIT - 216).unwrap();
    assert_eq!(layout.finish().unwrap().size, LIMIT);
    assert!(!layout.finish().unwrap().oversized());
    layout.member(1, 0).unwrap();
    assert!(layout.finish().unwrap().oversized());
    assert!(Layout::default().member(65_536, 0).is_err());
    assert!(Layout::default().member(0, 0).is_err());
    assert!(Layout::default().member(1, u64::MAX).is_err());
}
