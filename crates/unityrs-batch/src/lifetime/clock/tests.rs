use super::*;
fn date(year: u32, month: u32, day: u32) -> DateTime {
    DateTime {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        nanos: 0,
        offset_minutes: 0,
    }
}
#[test]
fn epoch_offsets_leaps_and_distant_dates() {
    assert_eq!(unix_ns(&date(1970, 1, 1)), Ok(0));
    assert_eq!(unix_ns(&date(1969, 12, 31)), Ok(-86_400_000_000_000));
    let mut leap = date(2016, 12, 31);
    leap.hour = 23;
    leap.minute = 59;
    leap.second = 60;
    assert_eq!(unix_ns(&leap), unix_ns(&date(2017, 1, 1)));
    leap = date(2017, 1, 1);
    leap.hour = 0;
    leap.minute = 59;
    leap.second = 60;
    leap.offset_minutes = 60;
    assert_eq!(unix_ns(&leap), unix_ns(&date(2017, 1, 1)));
    for d in [date(0, 1, 1), date(9999, 12, 31)] {
        let result = Policy::at(Some(&d), 10_000, 0, Duration::from_secs(1));
        if d.year == 0 {
            assert!(matches!(result, Err(TimingError::DeadlineTooClose)));
        } else {
            assert!(result.unwrap().deadline.unwrap().0 > 200_000_000_000_000);
        }
    }
    let mut invalid = date(2026, 1, 1);
    invalid.offset_minutes = i32::MIN;
    assert_eq!(unix_ns(&invalid), Err(TimingError::Date));
}
#[test]
fn cutoff_precision_overflow_and_minimum_grace_reserves() {
    let d = date(1970, 1, 2);
    let wall = unix_ns(&d).unwrap() - 10_000_999_999;
    assert!(matches!(
        Policy::at(Some(&d), 10_000, wall, Duration::ZERO),
        Err(TimingError::DeadlineTooClose)
    ));
    let policy = Policy::at(Some(&d), 10_000, wall - 1, Duration::ZERO).unwrap();
    assert_eq!(policy.deadline, Some(Boot(10_001)));
    assert_eq!(policy.cutoff, Some(Boot(1)));
    assert_eq!(policy.hard(Boot(0)), Ok(Boot(9500)));
    assert_eq!(Policy::non_head_cut(Boot(9500), false), Boot(6500));
    assert_eq!(Policy::head_cut(Boot(9500)), Boot(7500));
    assert_eq!(Policy::abort_ms(Boot(9500), Boot(6001), false), None);
    assert_eq!(Policy::abort_ms(Boot(9500), Boot(6000), false), Some(500));
    assert_eq!(policy.hard(Boot(u64::MAX - 1)), Err(TimingError::Overflow));
    assert_eq!(
        Boot::from_duration(Duration::MAX),
        Err(TimingError::Overflow)
    );
    assert!(matches!(
        Policy::at(Some(&d), 10_000, i128::MIN, Duration::ZERO),
        Err(TimingError::Overflow)
    ));
}
