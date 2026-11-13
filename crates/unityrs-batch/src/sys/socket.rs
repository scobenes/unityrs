use std::io;

/// Test-only inspection of a registered descriptor without taking ownership.
#[cfg(any(test, feature = "test-hooks"))]
#[must_use]
pub fn test_socket_live(descriptor: std::os::fd::RawFd) -> bool {
    // SAFETY: F_GETFD takes only an integer and touches no caller memory.
    // An already closed descriptor is the expected EBADF test outcome.
    unsafe { libc::fcntl(descriptor, libc::F_GETFD) != -1 }
}

/// Test-only proof that registration sees zero linger, before connect.
#[cfg(any(test, feature = "test-hooks"))]
pub fn test_zero_linger(descriptor: std::os::fd::RawFd) -> io::Result<bool> {
    let mut value = libc::linger {
        l_onoff: 0,
        l_linger: 0,
    };
    let mut len = libc::socklen_t::try_from(std::mem::size_of::<libc::linger>())
        .map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: value has space for one linger; len describes that writable
    // storage. It starts initialized even if libc returns a short result;
    // no ownership is taken.
    if unsafe {
        libc::getsockopt(
            descriptor,
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&raw mut value).cast(),
            &raw mut len,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(value.l_onoff != 0 && value.l_linger == 0)
}

/// Test-only reset fixture: set zero linger before closing the live socket.
#[cfg(any(test, feature = "test-hooks"))]
pub fn test_set_zero_linger(descriptor: std::os::fd::RawFd) -> io::Result<()> {
    let value = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    let length = libc::socklen_t::try_from(size_of::<libc::linger>())
        .map_err(|_| io::ErrorKind::InvalidInput)?;
    // SAFETY: value is fully initialized, length is its exact size, and
    // setsockopt borrows its storage without taking ownership of the fd.
    if unsafe {
        libc::setsockopt(
            descriptor,
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&raw const value).cast(),
            length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
