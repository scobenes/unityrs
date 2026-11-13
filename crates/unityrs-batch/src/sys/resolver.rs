//! Copy only the first sixteen NSS answers into Rust-owned fixed storage.
use crate::http::resolve::{Addresses, MAX_ADDRESSES};
use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV6},
};

struct AnswerList(*mut libc::addrinfo);
impl Drop for AnswerList {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this list is owned by exactly one successful getaddrinfo
            // call and has not been freed or modified since that call.
            unsafe { libc::freeaddrinfo(self.0) };
        }
    }
}

/// Resolve a NUL-terminated name without any Rust heap allocation. NSS owns
/// its C allocations; answers are copied into `Addresses`' fixed array.
pub(crate) fn resolve(name: &[u8], port: u16) -> io::Result<Addresses> {
    let name =
        std::ffi::CStr::from_bytes_with_nul(name).map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: addrinfo's integer and pointer fields all accept zero; the
    // remaining hint fields are explicitly set before libc reads them.
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_UNSPEC;
    hints.ai_socktype = libc::SOCK_STREAM;
    hints.ai_protocol = libc::IPPROTO_TCP;
    let mut head = std::ptr::null_mut();
    // SAFETY: CStr validated the caller-owned buffer's trailing and interior
    // NULs; its borrow stays live through the call. hints is initialized and
    // head is writable. No pointer into Rust storage is retained by libc.
    // A null service asks for port zero, which we replace with the caller's.
    let result = unsafe {
        libc::getaddrinfo(
            name.as_ptr(),
            std::ptr::null(),
            &raw const hints,
            &raw mut head,
        )
    };
    if result != 0 {
        return Err(match result {
            libc::EAI_MEMORY => io::ErrorKind::OutOfMemory.into(),
            libc::EAI_SYSTEM => io::Error::last_os_error(),
            _ => io::ErrorKind::NotFound.into(),
        });
    }
    let answers = AnswerList(head);
    // SAFETY: the guard owns this complete getaddrinfo list until copying
    // finishes, including any early return or unwinding in its caller.
    unsafe { copy_answers(answers.0, port) }
}

/// The caller keeps the first sixteen nodes and their sockaddr storage alive
/// and readable; a null pointer ends the list sooner.
unsafe fn copy_answers(mut next: *const libc::addrinfo, port: u16) -> io::Result<Addresses> {
    let entries = std::iter::from_fn(|| {
        if next.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees that next refers to a live list node.
        let entry = unsafe { &*next };
        next = entry.ai_next;
        Some(entry)
    });
    // The entry cap applies before decoding: unsupported families cannot make
    // us traverse an arbitrarily large list to find sixteen useful addresses.
    let addresses = Addresses::first(entries.take(MAX_ADDRESSES).filter_map(|entry| {
        // SAFETY: the caller owns the sockaddr described by this live node.
        unsafe { socket_address(entry, port) }
    }));
    if addresses.is_empty() {
        return Err(io::ErrorKind::NotFound.into());
    }
    Ok(addresses)
}

unsafe fn socket_address(entry: &libc::addrinfo, port: u16) -> Option<SocketAddr> {
    if entry.ai_addr.is_null() {
        return None;
    }
    let length = usize::try_from(entry.ai_addrlen).ok()?;
    match entry.ai_family {
        libc::AF_INET if length >= size_of::<libc::sockaddr_in>() => {
            // SAFETY: the caller guarantees readable sockaddr storage; its
            // family and checked length permit this full, unaligned copy.
            let address = unsafe { entry.ai_addr.cast::<libc::sockaddr_in>().read_unaligned() };
            Some((Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()), port).into())
        }
        libc::AF_INET6 if length >= size_of::<libc::sockaddr_in6>() => {
            // SAFETY: the checked IPv6 family and length permit this full,
            // unaligned copy from the caller's live sockaddr storage.
            let address = unsafe { entry.ai_addr.cast::<libc::sockaddr_in6>().read_unaligned() };
            Some(
                SocketAddrV6::new(
                    Ipv6Addr::from(address.sin6_addr.s6_addr),
                    port,
                    address.sin6_flowinfo,
                    address.sin6_scope_id,
                )
                .into(),
            )
        }
        _ => None,
    }
}

#[cfg(any(test, feature = "test-hooks"))]
#[path = "resolver/hooks.rs"]
mod hooks;
#[cfg(any(test, feature = "test-hooks"))]
pub use hooks::TestResolverAnswers;
#[cfg(test)]
#[path = "resolver/tests.rs"]
mod tests;
