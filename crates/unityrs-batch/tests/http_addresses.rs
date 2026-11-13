//! Fixed resolver output preserves duplicates, address families and order.
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use unityrs_batch::http::resolve::{Addresses, MAX_ADDRESSES};
#[test]
fn only_the_first_sixteen_answers_are_retained_in_resolver_order() {
    let source: Vec<_> = (0..32)
        .map(|n| {
            if n % 2 == 0 {
                SocketAddr::from((Ipv4Addr::LOCALHOST, n))
            } else {
                SocketAddr::from((Ipv6Addr::LOCALHOST, n))
            }
        })
        .collect();
    let addresses = Addresses::first(source.iter().copied());
    assert_eq!(addresses.len(), MAX_ADDRESSES);
    assert_eq!(addresses.iter().collect::<Vec<_>>(), source[..16]);
    let repeated = SocketAddr::from((Ipv4Addr::LOCALHOST, 80));
    assert_eq!(
        Addresses::first([repeated, repeated])
            .iter()
            .collect::<Vec<_>>(),
        [repeated, repeated]
    );
    let empty = Addresses::default();
    assert_eq!(empty.len(), 0);
    assert_eq!(empty.iter().count(), 0);
}
