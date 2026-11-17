//! Initial argument/environment strings and pointer slots, separate from the
//! main stack's fixed storage. Check mode always uses the Linux cap instead.
use super::CHECK_ENV_BLOCK;
#[cfg(any(target_os = "linux", test))]
use std::io::{self, Read};

#[cfg(any(target_os = "linux", test))]
fn block(mut reader: impl Read) -> Option<u64> {
    let mut bytes = 0_u64;
    let mut pointers = 0_u64;
    let mut terminated = true;
    let mut buffer = [0; 1024];
    loop {
        let n = match reader.read(&mut buffer) {
            Ok(0) => {
                return if terminated {
                    bytes.checked_add(pointers)
                } else {
                    None
                };
            }
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        bytes = bytes.checked_add(u64::try_from(n).ok()?)?;
        let part = buffer.get(..n)?;
        terminated = part.last() == Some(&0);
        for &byte in part {
            if byte == 0 {
                pointers = pointers.checked_add(size_of::<usize>() as u64)?;
            }
        }
        if bytes.checked_add(pointers)? > CHECK_ENV_BLOCK {
            return None;
        }
    }
}

/// Normal execution's inherited argument/environment block. Linux exposes
/// the initial strings without copying them into a heap collection. Count
/// their pointer slots too; fixed auxiliary-vector storage stays in the main
/// stack. An unavailable or oversized procfs read retains the conservative cap.
/// Non-Linux development hosts retain that cap and their page certification.
#[must_use]
pub fn execution_env_block() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let measured = || {
            let args = block(std::fs::File::open("/proc/self/cmdline").ok()?)?;
            let env = block(std::fs::File::open("/proc/self/environ").ok()?)?;
            args.checked_add(env)?
                .checked_add(2 * size_of::<usize>() as u64)
        };
        measured().unwrap_or(CHECK_ENV_BLOCK).min(CHECK_ENV_BLOCK)
    }
    #[cfg(not(target_os = "linux"))]
    {
        CHECK_ENV_BLOCK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_and_pointer_slots_are_counted_without_a_heap_collection() {
        assert_eq!(
            block(&b"a\0bc\0"[..]),
            Some(5 + 2 * size_of::<usize>() as u64)
        );
        assert_eq!(block(&b""[..]), Some(0));
        assert_eq!(block(&b"unterminated"[..]), None);
        assert!(block(io::repeat(0)).is_none());
    }
    #[test]
    fn unreadable_blocks_use_the_cap_and_success_never_exceeds_it() {
        struct Refused;
        impl Read for Refused {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::ErrorKind::PermissionDenied.into())
            }
        }
        assert_eq!(block(Refused), None);
        assert!(execution_env_block() <= CHECK_ENV_BLOCK);
        let (_, measured) = crate::alloc::measured(execution_env_block);
        assert_eq!(
            measured,
            crate::alloc::Measured::default(),
            "environment accounting allocated"
        );
        crate::test_alloc::REFUSE.with(|r| r.set(1));
        let value = execution_env_block();
        let armed = crate::test_alloc::REFUSE.with(|r| r.replace(0));
        assert_eq!(armed, 1, "environment accounting attempted an allocation");
        assert!(value <= CHECK_ENV_BLOCK);
    }
}
