//! One encrypted connection to another machine, run by a thread of its own.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use rustls::Connection;

use crate::proto::Framer;

const OUTBOX: usize = 512;
const FILE_SHARE: usize = OUTBOX / 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    Needed,
    Spare,
    Wait,
}

type OnFrame = dyn FnMut(&Arc<Link>, u8, Vec<u8>);

pub struct Link {
    outbox: Mutex<VecDeque<Vec<u8>>>,
    room: Condvar,
    alive: AtomicBool,
    heard: AtomicU64,
    wake: UnixStream,
    started: Instant,
}

impl Link {
    pub fn start(
        conn: Connection,
        sock: TcpStream,
        framer: Framer,
        leftover: Vec<(u8, Vec<u8>)>,
        on_frame: impl FnMut(&Arc<Link>, u8, Vec<u8>) + Send + 'static,
        on_close: impl FnOnce(String) + Send + 'static,
    ) -> std::io::Result<Arc<Link>> {
        let (wake, woken) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        woken.set_nonblocking(true)?;
        sock.set_nonblocking(true)?;
        let mut conn = conn;
        conn.set_buffer_limit(None);
        let link = Arc::new(Link {
            outbox: Mutex::new(VecDeque::new()),
            room: Condvar::new(),
            alive: AtomicBool::new(true),
            heard: AtomicU64::new(0),
            wake,
            started: Instant::now(),
        });
        let me = link.clone();
        std::thread::Builder::new().name("link".into()).spawn(move || {
            let mut on_frame = on_frame;
            for (k, b) in leftover {
                on_frame(&me, k, b);
            }
            let why = Link::pump(&me, conn, sock, framer, woken, &mut on_frame);
            me.close();
            on_close(why);
        })?;
        Ok(link)
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    pub fn quiet_for(&self) -> Duration {
        let at = Duration::from_millis(self.heard.load(Ordering::Relaxed));
        self.started.elapsed().saturating_sub(at)
    }

    pub fn touch(&self) {
        self.heard.store(self.started.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    pub fn send(&self, blob: Vec<u8>, urgency: Urgency) -> bool {
        if !self.alive() {
            return false;
        }
        let Ok(mut q) = self.outbox.lock() else { return false };
        if urgency == Urgency::Wait {
            let deadline = Instant::now() + Duration::from_secs(5);
            while q.len() >= FILE_SHARE {
                let left = deadline.saturating_duration_since(Instant::now());
                if !self.alive() || left.is_zero() {
                    return false;
                }
                q = match self.room.wait_timeout(q, left.min(Duration::from_millis(200))) {
                    Ok((g, _)) => g,
                    Err(_) => return false,
                };
            }
        } else if q.len() >= OUTBOX {
            if urgency == Urgency::Needed {
                self.alive.store(false, Ordering::Relaxed);
                let _ = (&self.wake).write(&[1]);
            }
            return false;
        }
        q.push_back(blob);
        drop(q);
        let _ = (&self.wake).write(&[1]);
        true
    }

    pub fn close(&self) {
        self.alive.store(false, Ordering::Relaxed);
        self.room.notify_all();
        let _ = (&self.wake).write(&[1]);
    }

    fn pump(
        me: &Arc<Link>,
        mut conn: Connection,
        mut sock: TcpStream,
        mut framer: Framer,
        mut woken: UnixStream,
        on_frame: &mut OnFrame,
    ) -> String {
        let mut buf = vec![0u8; 64 * 1024];
        let mut wait = 0;
        while me.alive() {
            let turn = me
                .send_waiting(&mut conn)
                .and_then(|()| flush(&mut conn, &mut sock))
                .and_then(|()| take_in(&mut conn, &mut sock, &mut woken, &mut buf, std::mem::replace(&mut wait, 1000)))
                .and_then(|()| hand_out(me, &mut conn, &mut framer, &mut buf, on_frame));
            if let Err(why) = turn {
                return why;
            }
        }
        "closed".into()
    }

    fn send_waiting(&self, conn: &mut Connection) -> Result<(), String> {
        if conn.wants_write() {
            return Ok(());
        }
        let Ok(mut q) = self.outbox.lock() else { return Ok(()) };
        let mut moved = 0;
        while let Some(blob) = q.pop_front() {
            if conn.writer().write_all(&blob).is_err() {
                return Err("could not send a whole message".into());
            }
            moved += blob.len();
            if moved > 256 * 1024 {
                break;
            }
        }
        self.room.notify_all();
        Ok(())
    }
}

fn flush(conn: &mut Connection, sock: &mut TcpStream) -> Result<(), String> {
    while conn.wants_write() {
        match conn.write_tls(sock) {
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn take_in(conn: &mut Connection, sock: &mut TcpStream, woken: &mut UnixStream, buf: &mut [u8], wait: i32) -> Result<(), String> {
    let mut fds = [
        libc::pollfd {
            fd: sock.as_raw_fd(),
            events: libc::POLLIN | if conn.wants_write() { libc::POLLOUT } else { 0 },
            revents: 0,
        },
        libc::pollfd { fd: woken.as_raw_fd(), events: libc::POLLIN, revents: 0 },
    ];
    // SAFETY: fds is a live array of two pollfd for the length given,
    // and both descriptors stay open for the call.
    let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, wait) };
    if n < 0 {
        if std::io::Error::last_os_error().kind() == ErrorKind::Interrupted {
            return Ok(());
        }
        return Err("poll failed".into());
    }
    if fds[1].revents != 0 {
        while woken.read(buf).is_ok_and(|n| n > 0) {}
    }
    if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
        match conn.read_tls(sock) {
            Ok(0) => return Err("the other machine closed the connection".into()),
            Ok(_) => {}
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(e) => return Err(e.to_string()),
        }
        if let Err(e) = conn.process_new_packets() {
            let _ = conn.write_tls(sock);
            return Err(e.to_string());
        }
    }
    Ok(())
}

fn hand_out(
    me: &Arc<Link>,
    conn: &mut Connection,
    framer: &mut Framer,
    buf: &mut [u8],
    on_frame: &mut OnFrame,
) -> Result<(), String> {
    loop {
        match conn.reader().read(buf) {
            Ok(0) => return Err("the other machine closed the connection".into()),
            Ok(n) => {
                me.touch();
                framer.feed(&buf[..n]).map_err(|e| e.to_string())?.into_iter().for_each(|(k, b)| on_frame(me, k, b));
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e.to_string()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::config::tests::TestHome;
    use crate::{proto, tls};
    use rustls::{ServerConnection, StreamOwned};
    use std::net::TcpListener;
    use std::sync::mpsc;

    pub(crate) fn pair() -> (Arc<Link>, Arc<Link>) {
        tls::ensure_cert().unwrap();
        let server = tls::server_config().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            tls::tune(&sock);
            let mut s = StreamOwned::new(ServerConnection::new(server).unwrap(), sock);
            while s.conn.is_handshaking() {
                s.conn.complete_io(&mut s.sock).unwrap();
            }
            let StreamOwned { conn, sock } = s;
            Link::start(conn.into(), sock, Framer::default(), Vec::new(), |_, _, _| {}, |_| {}).unwrap()
        });
        let (c, _) = tls::connect("127.0.0.1", port, Duration::from_secs(5)).unwrap();
        let StreamOwned { conn, sock } = c;
        let near = Link::start(conn.into(), sock, Framer::default(), Vec::new(), |_, _, _| {}, |_| {}).unwrap();
        (t.join().unwrap(), near)
    }

    #[test]
    fn a_silent_other_end_reads_as_quiet_until_it_says_something() {
        let _home = TestHome::new();
        let (far, near) = pair();
        near.touch();
        std::thread::sleep(Duration::from_millis(400));
        assert!(near.quiet_for() >= Duration::from_millis(400));
        assert!(far.send(proto::pack(proto::PING, b""), Urgency::Needed));
        let deadline = Instant::now() + Duration::from_secs(3);
        while near.quiet_for() >= Duration::from_millis(400) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(near.quiet_for() < Duration::from_millis(400));
        far.close();
        near.close();
    }

    #[test]
    fn connections_are_probed_by_the_system_when_quiet() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let sock = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        tls::tune(&sock);
        let fd = std::os::fd::AsRawFd::as_raw_fd(&sock);
        let get = |level, name| {
            let (mut value, mut len): (libc::c_int, libc::socklen_t) = (0, size_of::<libc::c_int>() as libc::socklen_t);
            // SAFETY: reads an int option of an open socket into a local int.
            unsafe { libc::getsockopt(fd, level, name, std::ptr::from_mut(&mut value).cast(), &mut len) };
            value
        };
        assert_eq!(get(libc::SOL_SOCKET, libc::SO_KEEPALIVE), 1);
        assert_eq!(get(libc::IPPROTO_TCP, libc::TCP_KEEPIDLE), 10);
        assert_eq!(get(libc::IPPROTO_TCP, libc::TCP_KEEPCNT), 3);
    }

    #[test]
    fn a_big_message_arrives_whole_and_so_does_the_next() {
        let _home = TestHome::new();
        tls::ensure_cert().unwrap();
        let server = tls::server_config().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (got_tx, got_rx) = mpsc::channel::<(u8, usize, u8)>();
        let t = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let mut s = StreamOwned::new(ServerConnection::new(server).unwrap(), sock);
            let mut hello = [0u8; 5];
            s.read_exact(&mut hello).unwrap();
            s.write_all(&hello).unwrap();
            s.flush().unwrap();
            let StreamOwned { conn, sock } = s;
            let link = Link::start(
                conn.into(),
                sock,
                Framer::default(),
                Vec::new(),
                move |_, kind, body| {
                    let _ = got_tx.send((kind, body.len(), body.last().copied().unwrap_or(0)));
                },
                |_| {},
            )
            .unwrap();
            std::thread::sleep(Duration::from_secs(4));
            link.close();
        });
        let (mut c, _) = tls::connect("127.0.0.1", port, Duration::from_secs(5)).unwrap();
        c.write_all(b"hello").unwrap();
        let mut back = [0u8; 5];
        c.read_exact(&mut back).unwrap();
        let StreamOwned { conn, sock } = c;
        let link = Link::start(conn.into(), sock, Framer::default(), Vec::new(), |_, _, _| {}, |_| {}).unwrap();
        let mut picture = vec![7u8; 3 * 1024 * 1024];
        *picture.last_mut().unwrap() = 9;
        assert!(link.send(proto::pack(proto::PICTURE, &picture), Urgency::Spare));
        assert!(link.send(proto::pack(proto::CLIPBOARD, b"after"), Urgency::Needed));
        let first = got_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second = got_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(first, (proto::PICTURE, 3 * 1024 * 1024, 9));
        assert_eq!(second, (proto::CLIPBOARD, 5, b'r'));
        link.close();
        t.join().unwrap();
    }
}
