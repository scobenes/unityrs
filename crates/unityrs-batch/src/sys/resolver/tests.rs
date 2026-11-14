use super::{copy_answers, resolve};
use crate::http::resolve::MAX_ADDRESSES;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

// Counted by the library test binary's one allocator (`crate::test_alloc`).

#[test]
fn system_lookup_uses_no_rust_heap() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let (answers, measured) = runtime
        .block_on(
            runtime.spawn_blocking(|| crate::alloc::measured(|| resolve(b"127.0.0.1\0", 12_345))),
        )
        .unwrap();
    let answers = answers.unwrap();
    assert!(!answers.is_empty());
    assert!(
        answers
            .iter()
            .all(|address| address == SocketAddr::from(([127, 0, 0, 1], 12_345)))
    );
    assert_eq!(measured.peak, 0, "System resolver allocated Rust storage");
}

#[test]
fn real_lookup_preserves_numeric_families_and_ports() {
    for address in [Ipv4Addr::LOCALHOST.into(), Ipv6Addr::LOCALHOST.into()] {
        let expected = SocketAddr::new(address, 12_345);
        let mut name = address.to_string().into_bytes();
        name.push(0);
        let answers = resolve(&name, 12_345).unwrap();
        assert!(!answers.is_empty());
        assert!(answers.iter().all(|actual| actual == expected));
    }
    for name in [b"".as_slice(), b"unterminated", b"bad\0host\0"] {
        assert_eq!(
            resolve(name, 80).unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}

#[test]
fn only_the_first_sixteen_nodes_are_inspected() {
    // The last first-sixteen node has a dangling tail. Inspecting that tail
    // would violate the cap even though unsupported entries produce no output.
    // SAFETY: addrinfo's integer and pointer fields all accept zero bits.
    let mut nodes: [libc::addrinfo; MAX_ADDRESSES] = unsafe { std::mem::zeroed() };
    for index in 0..MAX_ADDRESSES - 1 {
        nodes[index].ai_next = &raw mut nodes[index + 1];
    }
    nodes[MAX_ADDRESSES - 1].ai_next = std::ptr::dangling_mut();
    // SAFETY: the first sixteen nodes are initialized and live. copy_answers
    // must cap traversal before dereferencing the deliberately invalid tail.
    let result = unsafe { copy_answers(nodes.as_ptr(), 80) };
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn sockaddr_lengths_are_checked_before_copying() {
    // SAFETY: addrinfo has only integer and pointer fields, all zero valid.
    let mut node: libc::addrinfo = unsafe { std::mem::zeroed() };
    node.ai_family = libc::AF_INET;
    node.ai_addr = std::ptr::dangling_mut();
    node.ai_addrlen = 0;
    // SAFETY: the sole list node is live and initialized. Its zero length
    // means its deliberately invalid sockaddr must never be dereferenced.
    let result = unsafe { copy_answers(&raw const node, 80) };
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
}
