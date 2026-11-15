//! Bounded live connections; a refused connection closes without a worker.
use super::{Arc, Server, TcpStream, io};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
struct Permit(Arc<AtomicUsize>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, SeqCst);
    }
}

type Job = Box<dyn FnOnce() + Send>;
pub(super) const WORKERS: usize = 32;

pub(super) fn serve(
    server: &Arc<Server>,
    incoming: impl Iterator<Item = io::Result<TcpStream>>,
    limit: usize,
    mut spawn: impl FnMut(Job) -> io::Result<()>,
) -> io::Result<()> {
    let active = Arc::new(AtomicUsize::new(0));
    for stream in incoming {
        let stream = stream?;
        if active
            .try_update(SeqCst, SeqCst, |n| (n < limit).then_some(n + 1))
            .is_err()
        {
            // Dropping refuses without allocating a worker or writing to a slow peer.
            continue;
        }
        let permit = Permit(active.clone());
        let server = server.clone();
        if let Err(e) = spawn(Box::new(move || {
            let _permit = permit;
            if let Err(e) = server.serve(stream) {
                eprintln!("CONNECTION error={:?}", e.kind());
            }
        })) {
            // A failed spawn drops the closure and returns its permit.
            eprintln!("SPAWN error={:?}", e.kind());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::{Ipv4Addr, TcpListener},
        sync::mpsc,
    };
    use unityrs_testkit::{guard::DEADLOCK_GUARD, scratch::Scratch};
    fn connection() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client.set_read_timeout(Some(DEADLOCK_GUARD)).unwrap();
        (client, listener.accept().unwrap().0)
    }
    #[test]
    fn slow_connections_are_refused_at_capacity_and_capacity_is_reclaimed() {
        let scratch = Scratch::new("serve-admission");
        let server = Arc::new(super::super::tests::server(scratch.path(), &[]));
        let (first, a) = connection();
        let (second, b) = connection();
        let (mut refused, c) = connection();
        let (mut resumed, d) = connection();
        write!(resumed, "GET /missing HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut last = Some(d);
        let (done_tx, done_rx) = mpsc::channel();
        let mut workers = Vec::new();
        let started = std::sync::atomic::AtomicUsize::new(0);
        let mut connections = vec![Ok(a), Ok(b), Ok(c)].into_iter();
        // The last iterator step observes refusal before freeing a worker.
        let incoming = std::iter::from_fn(|| {
            if let Some(stream) = connections.next() {
                return Some(stream);
            }
            let d = last.take()?;
            assert_eq!(
                started.load(std::sync::atomic::Ordering::SeqCst),
                2,
                "admitted past capacity"
            );
            let mut byte = [0];
            assert_eq!(refused.read(&mut byte).unwrap(), 0);
            first.shutdown(std::net::Shutdown::Both).unwrap();
            done_rx.recv_timeout(DEADLOCK_GUARD).unwrap();
            Some(Ok(d))
        });
        serve(&server, incoming, 2, |job| {
            started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let done = done_tx.clone();
            workers.push(std::thread::spawn(move || {
                job();
                let _ = done.send(());
            }));
            Ok(())
        })
        .unwrap();
        drop(second);
        for worker in workers {
            worker.join().unwrap();
        }
        let mut response = String::new();
        resumed.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 404"));
    }
    #[test]
    fn a_spawn_failure_does_not_end_service_or_consume_capacity() {
        let scratch = Scratch::new("serve-spawn");
        let server = Arc::new(super::super::tests::server(scratch.path(), &[]));
        let (mut failed, a) = connection();
        let (mut client, b) = connection();
        write!(client, "GET /missing HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut attempts = 0;
        let mut worker = None;
        serve(&server, [Ok(a), Ok(b)].into_iter(), 1, |job| {
            attempts += 1;
            if attempts == 1 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            worker = Some(std::thread::spawn(job));
            Ok(())
        })
        .unwrap();
        assert_eq!(failed.read(&mut [0]).unwrap(), 0);
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 404"));
        worker.unwrap().join().unwrap();
    }
}
