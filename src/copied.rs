//! The files the person just copied, handed over by Trindle through a socket.

use std::fs::File;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::time::Duration;

use crate::files::{Log, MAX_BYTES, MAX_FILES};

pub struct Copy {
    pub name: String,
    pub size: u64,
    pub file: File,
}

pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("TRINDLE_COPIED_SOCKET").filter(|p| !p.is_empty()).map(PathBuf::from)
}

fn connect(path: &std::path::Path) -> Result<OwnedFd, String> {
    use std::os::unix::ffi::OsStrExt;
    // SAFETY: plain socket calls on a descriptor owned by the OwnedFd, and a
    // sockaddr_un filled in whole and shorter than its path field.
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err("cannot make a socket to ask for the copied files".into());
        }
        let owned = OwnedFd::from_raw_fd(fd);
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        let bytes = path.as_os_str().as_bytes();
        if bytes.len() >= addr.sun_path.len() {
            return Err("the copied files socket has too long a name".into());
        }
        for (i, b) in bytes.iter().enumerate() {
            addr.sun_path[i] = *b as libc::c_char;
        }
        let tv = libc::timeval { tv_sec: 15, tv_usec: 0 };
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            (&tv as *const libc::timeval).cast(),
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
        let rc = libc::connect(
            fd,
            (&addr as *const libc::sockaddr_un).cast(),
            std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
        );
        if rc != 0 {
            return Err("Trindle did not answer when asked for the copied files".into());
        }
        Ok(owned)
    }
}

fn send_packet(fd: &OwnedFd, text: &str) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    let mut packet = (text.len() as u32).to_be_bytes().to_vec();
    packet.extend_from_slice(text.as_bytes());
    // SAFETY: the buffer is valid for its whole length.
    let sent = unsafe { libc::send(fd.as_raw_fd(), packet.as_ptr().cast(), packet.len(), libc::MSG_NOSIGNAL) };
    if sent == packet.len() as isize {
        Ok(())
    } else {
        Err("could not ask Trindle for the copied files".into())
    }
}

fn receive(fd: &OwnedFd) -> Result<(String, Vec<OwnedFd>), String> {
    use std::os::fd::AsRawFd;
    let mut data = vec![0u8; 1 << 20];
    // u64s, so the buffer has the alignment cmsghdr needs when read through.
    let mut control = vec![0u64; 1024];
    let mut iov = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: data.len() };
    // SAFETY: msghdr is filled with pointers to buffers that outlive the call,
    // the control buffer is aligned for cmsghdr, and every descriptor the kernel passes is wrapped at once so it is
    // closed whatever happens next.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = std::mem::size_of_val(control.as_slice()) as _;
        let got = libc::recvmsg(fd.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC);
        let mut fds = Vec::new();
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let n = ((*c).cmsg_len as usize).saturating_sub(libc::CMSG_LEN(0) as usize) / std::mem::size_of::<RawFd>();
                let p = libc::CMSG_DATA(c) as *const RawFd;
                for i in 0..n {
                    fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(p.add(i))));
                }
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        if got < 4 {
            return Err("Trindle did not answer when asked for the copied files".into());
        }
        let got = got as usize;
        let len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if len + 4 > got {
            return Err("Trindle sent a broken answer about the copied files".into());
        }
        Ok((String::from_utf8_lossy(&data[4..4 + len]).into_owned(), fds))
    }
}

#[derive(Debug)]
pub enum Refused {
    SlowDown,
    Nothing,
    Failed(String),
}

impl From<String> for Refused {
    fn from(e: String) -> Refused {
        Refused::Failed(e)
    }
}

impl From<&str> for Refused {
    fn from(e: &str) -> Refused {
        Refused::Failed(e.into())
    }
}

impl Refused {
    pub fn message(&self) -> String {
        match self {
            Refused::SlowDown => why("slow_down"),
            Refused::Nothing => String::new(),
            Refused::Failed(e) => e.clone(),
        }
    }
}

fn why(code: &str) -> String {
    match code {
        "not_allowed" => "Flow has not been allowed to read the files you copy. Allow it in Settings, Permissions.",
        "slow_down" => "files were copied twice within two seconds, so the second was not sent",
        "unavailable" => "Trindle cannot hand over copied files just now",
        "too_old" => "the files were copied too long ago to be sent",
        "no_files" => "no files are copied",
        "too_big" => "the files copied are too big to send",
        "busy" => "Trindle is busy handing over other copied files",
        _ => "Trindle did not understand Flow's request for the copied files",
    }
    .to_string()
}

pub fn fetch(path: &std::path::Path, log: &Log) -> Result<Vec<Copy>, Refused> {
    let fd = connect(path)?;
    send_packet(&fd, "LIST")?;
    let mut out: Vec<Copy> = Vec::new();
    let mut skipped = 0usize;
    loop {
        let (text, mut fds) = receive(&fd)?;
        let mut lines = text.lines();
        let first = lines.next().unwrap_or("");
        if let Some(code) = first.strip_prefix("ERR\t") {
            return Err(match code.trim() {
                "slow_down" => Refused::SlowDown,
                code => Refused::Failed(why(code)),
            });
        }
        if first.starts_with("DONE") {
            break;
        }
        if first != "FILES" {
            return Err("Trindle sent a broken answer about the copied files".into());
        }
        fds.reverse();
        for line in lines {
            let mut part = line.split('\t');
            let (kind, size, rel) = (part.next(), part.next(), part.next());
            if kind != Some("F") {
                skipped += 1;
                continue;
            }
            let Some(taken) = fds.pop() else { return Err("Trindle sent fewer files than it listed".into()) };
            let (Some(size), Some(rel)) = (size.and_then(|s| s.parse::<u64>().ok()), rel) else { continue };
            if rel.contains('/') || size > MAX_BYTES || out.len() >= MAX_FILES {
                skipped += 1;
                continue;
            }
            out.push(Copy { name: rel.to_string(), size, file: File::from(taken) });
        }
    }
    if skipped > 0 {
        log(format!(
            "file: {skipped} left behind: folders, or too big (the limit is {} MB)",
            MAX_BYTES / (1024 * 1024)
        ));
    }
    if out.is_empty() {
        return Err(if skipped > 0 { Refused::Nothing } else { Refused::Failed(why("no_files")) });
    }
    Ok(out)
}

pub const RETRY: Duration = Duration::from_millis(2200);

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    use std::sync::Arc;

    fn pipe_with(bytes: &[u8]) -> OwnedFd {
        let mut fds = [0; 2];
        // SAFETY: pipe fills two descriptors we then own.
        let (r, w) = unsafe {
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            (OwnedFd::from_raw_fd(fds[0]), File::from(OwnedFd::from_raw_fd(fds[1])))
        };
        let mut w = w;
        w.write_all(bytes).unwrap();
        r
    }

    fn serve(path: PathBuf) {
        // SAFETY: plain socket calls on descriptors owned here.
        unsafe {
            let l = libc::socket(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0);
            let mut addr: libc::sockaddr_un = std::mem::zeroed();
            addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
            for (i, b) in path.as_os_str().as_bytes().iter().enumerate() {
                addr.sun_path[i] = *b as libc::c_char;
            }
            assert_eq!(libc::bind(l, (&addr as *const libc::sockaddr_un).cast(), std::mem::size_of::<libc::sockaddr_un>() as u32), 0);
            assert_eq!(libc::listen(l, 1), 0);
            std::thread::spawn(move || {
                let c = libc::accept(l, std::ptr::null_mut(), std::ptr::null_mut());
                let mut buf = [0u8; 64];
                let n = libc::recv(c, buf.as_mut_ptr().cast(), buf.len(), 0);
                assert_eq!(&buf[4..n as usize], b"LIST");
                let a = pipe_with(b"hello");
                let b = pipe_with(b"second file");
                let text = "FILES\nF\t5\ta.txt\nD\t0\tfolder\nF\t7\tfolder/inner.txt\nF\t11\tb.txt\n";
                let mut packet = (text.len() as u32).to_be_bytes().to_vec();
                packet.extend_from_slice(text.as_bytes());
                let inner = pipe_with(b"inner!!");
                let fds = [a.as_raw_fd(), inner.as_raw_fd(), b.as_raw_fd()];
                let mut iov = libc::iovec { iov_base: packet.as_mut_ptr().cast(), iov_len: packet.len() };
                let space = libc::CMSG_SPACE(std::mem::size_of_val(&fds) as u32) as usize;
                let mut control = vec![0u64; space.div_ceil(8)];
                let mut msg: libc::msghdr = std::mem::zeroed();
                msg.msg_iov = &mut iov;
                msg.msg_iovlen = 1;
                msg.msg_control = control.as_mut_ptr().cast();
                msg.msg_controllen = space as _;
                let h = libc::CMSG_FIRSTHDR(&msg);
                (*h).cmsg_level = libc::SOL_SOCKET;
                (*h).cmsg_type = libc::SCM_RIGHTS;
                (*h).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(&fds) as u32) as _;
                std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(h) as *mut RawFd, 3);
                assert!(libc::sendmsg(c, &msg, 0) > 0);
                let done = b"DONE\tcopy\t3\t23\t1";
                let mut p = (done.len() as u32).to_be_bytes().to_vec();
                p.extend_from_slice(done);
                libc::send(c, p.as_ptr().cast(), p.len(), 0);
                std::thread::sleep(Duration::from_millis(300));
                libc::close(c);
                libc::close(l);
            });
        }
    }

    #[test]
    fn copied_files_arrive_as_open_files_and_folders_are_left() {
        let mut b = [0u8; 8];
        let _ = crate::tls::random(&mut b);
        let path = std::env::temp_dir().join(format!("flow-copied-{:x}", u64::from_le_bytes(b)));
        serve(path.clone());
        let said = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log: Log = {
            let said = said.clone();
            Arc::new(move |l| said.lock().unwrap().push(l))
        };
        let got = fetch(&path, &log).unwrap();
        let names: Vec<_> = got.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a.txt", "b.txt"]);
        let mut text = String::new();
        let mut files = got;
        files[1].file.read_to_string(&mut text).unwrap();
        assert_eq!(text, "second file");
        assert!(said.lock().unwrap().iter().any(|l| l.contains("2 left behind")));
        let _ = std::fs::remove_file(path);
    }
}
