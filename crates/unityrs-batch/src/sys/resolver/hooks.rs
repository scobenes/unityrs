//! A synthetic NSS-owned linked list; setup is outside the counted lookup.
use super::{Addresses, copy_answers};
use std::io;

/// Immutable oversized answer storage for the resolver's counting regression.
/// Only the test build can construct or inspect this list.
pub struct TestResolverAnswers {
    nodes: Vec<libc::addrinfo>,
    _addresses: Vec<libc::sockaddr_in>,
}
// SAFETY: both vectors own their fixed allocations; every node points only
// into these allocations. Moving the vectors never moves their contents, and
// no method mutates either allocation after construction.
unsafe impl Send for TestResolverAnswers {}
// SAFETY: resolve only reads immutable nodes and sockaddrs. No raw pointer or
// mutable view escapes, and the owned vectors remain live for every borrow.
unsafe impl Sync for TestResolverAnswers {}
impl TestResolverAnswers {
    /// Build `count` IPv4 answers in order, each with its index modulo 256 in the last octet.
    pub fn new(count: usize) -> io::Result<Self> {
        let mut nodes = Vec::new();
        let mut addresses = Vec::new();
        nodes
            .try_reserve_exact(count)
            .map_err(|_| io::ErrorKind::OutOfMemory)?;
        addresses
            .try_reserve_exact(count)
            .map_err(|_| io::ErrorKind::OutOfMemory)?;
        for index in 0..count {
            // SAFETY: sockaddr_in's fields are integers, all zero bits valid.
            let mut address: libc::sockaddr_in = unsafe { std::mem::zeroed() };
            address.sin_family = libc::AF_INET
                .try_into()
                .map_err(|_| io::ErrorKind::InvalidInput)?;
            let octet = u8::try_from(index % 256).map_err(|_| io::ErrorKind::InvalidInput)?;
            address.sin_addr.s_addr = u32::from_ne_bytes([127, 0, 0, octet]);
            addresses.push(address);
            // SAFETY: addrinfo has only integer and pointer fields; all zero
            // bits are valid before its family, size and pointers are set.
            nodes.push(unsafe { std::mem::zeroed::<libc::addrinfo>() });
        }
        let mut next = std::ptr::null_mut();
        for (node, address) in nodes.iter_mut().zip(addresses.iter_mut()).rev() {
            node.ai_family = libc::AF_INET;
            node.ai_addrlen = libc::socklen_t::try_from(size_of::<libc::sockaddr_in>())
                .map_err(|_| io::ErrorKind::InvalidInput)?;
            node.ai_addr = std::ptr::from_mut(address).cast();
            node.ai_next = next;
            next = std::ptr::from_mut(node);
        }
        Ok(Self {
            nodes,
            _addresses: addresses,
        })
    }

    /// Exercise exactly the production list walker without calling DNS.
    pub fn resolve(&self) -> io::Result<Addresses> {
        let head = self
            .nodes
            .first()
            .map_or(std::ptr::null(), std::ptr::from_ref);
        // SAFETY: every node and sockaddr stays owned, immutable and reachable
        // for this borrow; constructing the fixture validated all their sizes.
        unsafe { copy_answers(head, 80) }
    }
}
