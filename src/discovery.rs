//! Finding the other computers.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::tls::hex;

pub const PORT: u16 = 24811;
pub const MAGIC: &str = "lmflow";
const INTERVAL: Duration = Duration::from_secs(2);
const STALE_AFTER: Duration = Duration::from_secs(8);
const LONGEST_PAUSE: u32 = 7;
const SWEEP_BATCH: usize = 48;

pub fn machine_id() -> String {
    for p in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(raw) = std::fs::read_to_string(p) {
            let raw = raw.trim();
            if !raw.is_empty() {
                return hex(&crate::tls::sha256(raw.as_bytes()))[..16].to_string();
            }
        }
    }
    hex(&crate::tls::sha256(machine_name().as_bytes()))[..16].to_string()
}

pub fn machine_name() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its whole length, and gethostname
    // writes at most that many bytes into it.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return "computer".to_string();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(&buf[..end]).into_owned()
}

pub fn interfaces() -> Vec<(Ipv4Addr, Ipv4Addr, Option<Ipv4Addr>)> {
    let mut out = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list it allocated, or fails and
    // leaves it alone.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let v4 = |p: *const libc::sockaddr| -> Option<Ipv4Addr> {
        // SAFETY: p is either null or points at a sockaddr owned by the list,
        // alive until freeifaddrs; its family says whether it is a sockaddr_in.
        unsafe {
            if p.is_null() || i32::from((*p).sa_family) != libc::AF_INET {
                return None;
            }
            let sin = &*p.cast::<libc::sockaddr_in>();
            Some(Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr)))
        }
    };
    let mut cur = head;
    while !cur.is_null() {
        // SAFETY: cur is a node of the list getifaddrs returned, not yet freed.
        let ifa = unsafe { &*cur };
        let up = ifa.ifa_flags & (libc::IFF_UP as u32) != 0;
        if let (true, Some(a), Some(m)) = (up, v4(ifa.ifa_addr), v4(ifa.ifa_netmask)) {
            let b = if ifa.ifa_flags & (libc::IFF_BROADCAST as u32) != 0 { v4(ifa.ifa_ifu) } else { None };
            out.push((a, m, b));
        }
        cur = ifa.ifa_next;
    }
    // SAFETY: head came from getifaddrs and is freed exactly once.
    unsafe { libc::freeifaddrs(head) };
    out
}

fn broadcast_addresses() -> Vec<Ipv4Addr> {
    let mut v = vec![Ipv4Addr::BROADCAST, Ipv4Addr::LOCALHOST];
    for (_, _, b) in interfaces() {
        if let Some(b) = b {
            if !b.is_unspecified() && !b.is_broadcast() && !v.contains(&b) {
                v.push(b);
            }
        }
    }
    v
}

fn private(a: Ipv4Addr) -> bool {
    a.is_private() || a.is_link_local()
}

fn sweep_pause(empty_sweeps: u32) -> Duration {
    INTERVAL * (1 << empty_sweeps.min(LONGEST_PAUSE))
}

fn sweep_list() -> Vec<Ipv4Addr> {
    sweep_of(&interfaces())
}

fn sweep_of(found: &[(Ipv4Addr, Ipv4Addr, Option<Ipv4Addr>)]) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for &(addr, mask, _) in found {
        let m = u32::from(mask).max(0xFFFF_FF00);
        let size = (!m).wrapping_add(1);
        if !private(addr) || size <= 2 {
            continue;
        }
        let net = u32::from(addr) & m;
        for h in 1..size - 1 {
            let host = Ipv4Addr::from(net + h);
            if host != addr {
                out.push(host);
            }
        }
    }
    out
}

fn shared_socket(port: u16) -> std::io::Result<UdpSocket> {
    use std::os::fd::FromRawFd;
    // SAFETY: plain socket creation; the result is checked before use.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is a socket this function just made and nobody else owns;
    // UdpSocket takes ownership and closes it.
    let sock = unsafe { UdpSocket::from_raw_fd(fd) };
    let one: libc::c_int = 1;
    for opt in [libc::SO_REUSEADDR, libc::SO_REUSEPORT] {
        // SAFETY: `one` is a live c_int and the length given is its size.
        let rc = unsafe {
            libc::setsockopt(fd, libc::SOL_SOCKET, opt, (&one as *const libc::c_int).cast(), size_of::<libc::c_int>() as u32)
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    let addr = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: port.to_be(),
        sin_addr: libc::in_addr { s_addr: 0 },
        sin_zero: [0; 8],
    };
    // SAFETY: addr is a complete sockaddr_in and the length is its size.
    let rc = unsafe { libc::bind(fd, (&addr as *const libc::sockaddr_in).cast(), size_of::<libc::sockaddr_in>() as u32) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    sock.set_broadcast(true)?;
    Ok(sock)
}

fn may_answer(from: SocketAddr) -> bool {
    matches!(from, SocketAddr::V4(v4) if private(*v4.ip()) || v4.ip().is_loopback())
}

pub fn may_connect(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => private(a) || a.is_loopback() || (a.octets()[0] == 100 && a.octets()[1] & 0xC0 == 64),
        IpAddr::V6(a) => a.is_loopback(),
    }
}

#[derive(Default)]
struct ReplyLimit {
    since: Option<Instant>,
    total: u32,
    per: HashMap<IpAddr, u32>,
}

impl ReplyLimit {
    fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        if self.since.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1)) {
            *self = ReplyLimit { since: Some(now), ..Default::default() };
        }
        let n = self.per.entry(ip).or_default();
        if self.total >= 8 || *n >= 2 {
            return false;
        }
        self.total += 1;
        *n += 1;
        true
    }
}

fn send_quietly(sock: &UdpSocket, data: &[u8], to: Ipv4Addr) {
    use std::os::fd::AsRawFd;
    let addr = libc::sockaddr_in {
        sin_family: libc::AF_INET as libc::sa_family_t,
        sin_port: PORT.to_be(),
        sin_addr: libc::in_addr { s_addr: u32::from(to).to_be() },
        sin_zero: [0; 8],
    };
    // SAFETY: data and addr outlive the call; MSG_DONTWAIT means an address
    // with nobody behind it never holds up listening for the one that is.
    unsafe {
        libc::sendto(
            sock.as_raw_fd(),
            data.as_ptr().cast(),
            data.len(),
            libc::MSG_DONTWAIT,
            (&addr as *const libc::sockaddr_in).cast(),
            size_of::<libc::sockaddr_in>() as u32,
        );
    }
}

fn is_question(v: &Value) -> bool {
    v["app"] == MAGIC && v.get("q").is_some_and(|q| !q.is_null() && q != &json!(0) && q != &json!(false))
}

type Seen = Arc<Mutex<HashMap<String, (Value, Instant)>>>;

const MOST_FOUND: usize = 32;

pub fn clean_id(raw: &str) -> String {
    raw.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').take(32).collect()
}

fn note(found: &mut HashMap<String, (Value, Instant)>, mut msg: Value, from: SocketAddr, own: &str) -> bool {
    let id = msg["id"].as_str().unwrap_or_default().to_string();
    if msg["app"] != MAGIC || id.is_empty() || clean_id(&id) != id || msg.get("q").is_some() || id == own {
        return false;
    }
    found.retain(|_, (_, t)| t.elapsed() < STALE_AFTER);
    if found.len() >= MOST_FOUND && !found.contains_key(&id) {
        return false;
    }
    if let Some(name) = msg["name"].as_str() {
        msg["name"] = json!(crate::page::plain(name, 40));
    }
    if let SocketAddr::V4(v4) = from {
        msg["host"] = json!(v4.ip().to_string());
    }
    found.insert(id, (msg, Instant::now()));
    true
}

pub struct Seeker {
    stop: Arc<AtomicBool>,
    seen: Seen,
}

impl Seeker {
    pub fn start(sweep: bool, log: impl Fn(String) + Send + 'static) -> Seeker {
        let stop = Arc::new(AtomicBool::new(false));
        let seen: Seen = Arc::default();
        let (flag, found) = (stop.clone(), seen.clone());
        let own = machine_id();
        std::thread::spawn(move || {
            let sock = match UdpSocket::bind("0.0.0.0:0").and_then(|s| s.set_broadcast(true).map(|()| s)) {
                Ok(s) => s,
                Err(e) => {
                    log(format!("discovery: cannot look for computers ({e})"));
                    return;
                }
            };
            let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
            let question = json!({"app": MAGIC, "q": 1}).to_string();
            let mut last_ask: Option<Instant> = None;
            let mut pending: Vec<Ipv4Addr> = Vec::new();
            let (mut empty_sweeps, mut next_sweep) = (0u32, Instant::now());
            let mut buf = [0u8; 4096];
            while !flag.load(Ordering::Relaxed) {
                let nobody = found.lock().map(|s| s.is_empty()).unwrap_or(true);
                if last_ask.is_none_or(|t| t.elapsed() > INTERVAL) {
                    last_ask = Some(Instant::now());
                    for t in broadcast_addresses() {
                        send_quietly(&sock, question.as_bytes(), t);
                    }
                    if sweep && nobody && pending.is_empty() && last_ask.is_some_and(|t| t >= next_sweep) {
                        pending = sweep_list();
                        next_sweep = Instant::now() + sweep_pause(empty_sweeps);
                        empty_sweeps += 1;
                    }
                }
                if !nobody {
                    pending.clear();
                    (empty_sweeps, next_sweep) = (0, Instant::now());
                }
                if nobody && !pending.is_empty() {
                    let take = pending.len().min(SWEEP_BATCH);
                    for a in pending.drain(..take) {
                        send_quietly(&sock, question.as_bytes(), a);
                    }
                }
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                let Ok(msg) = serde_json::from_slice::<Value>(&buf[..n]) else { continue };
                if let Ok(mut s) = found.lock() {
                    note(&mut s, msg, from, &own);
                }
            }
        });
        Seeker { stop, seen }
    }

    pub fn found(&self, role: Option<&str>) -> Vec<Value> {
        let mut v: Vec<Value> = self
            .seen
            .lock()
            .map(|s| s.values().filter(|(_, t)| t.elapsed() < STALE_AFTER).map(|(m, _)| m.clone()).collect())
            .unwrap_or_default();
        if let Some(r) = role {
            v.retain(|m| m["role"] == r);
        }
        v.sort_by(|a, b| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")));
        v
    }
}

impl Drop for Seeker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub struct Responder {
    stop: Arc<AtomicBool>,
}

impl Responder {
    pub fn start(payload: impl Fn() -> Value + Send + 'static, log: impl Fn(String) + Send + 'static) -> Responder {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            let sock = match shared_socket(PORT) {
                Ok(s) => s,
                Err(e) => {
                    log(format!("discovery: cannot listen for questions ({e})"));
                    return;
                }
            };
            let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
            let mut last_shout: Option<Instant> = None;
            let mut limit = ReplyLimit::default();
            let mut buf = [0u8; 4096];
            while !flag.load(Ordering::Relaxed) {
                if let Ok((n, from)) = sock.recv_from(&mut buf) {
                    let msg: Value = serde_json::from_slice(&buf[..n]).unwrap_or(Value::Null);
                    if is_question(&msg) && may_answer(from) && limit.allow(from.ip(), Instant::now()) {
                        let _ = sock.send_to(payload().to_string().as_bytes(), from);
                    }
                }
                if last_shout.is_none_or(|t| t.elapsed() > INTERVAL) {
                    last_shout = Some(Instant::now());
                    let blob = payload().to_string();
                    for t in broadcast_addresses() {
                        let _ = sock.send_to(blob.as_bytes(), SocketAddrV4::new(t, PORT));
                    }
                }
            }
        });
        Responder { stop }
    }
}

impl Drop for Responder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_and_tailscale_sources_may_connect() {
        let ok = |s: &str| may_connect(s.parse().unwrap());
        assert!(ok("192.168.1.5") && ok("10.0.0.1") && ok("169.254.1.1") && ok("127.0.0.1") && ok("100.100.1.1") && ok("::1"));
        assert!(!ok("8.8.8.8") && !ok("100.63.0.1") && !ok("100.128.0.1") && !ok("2001:db8::1"));
    }

    #[test]
    fn replies_are_limited_overall_and_per_source() {
        let (mut l, t) = (ReplyLimit::default(), Instant::now());
        let a: IpAddr = "192.168.1.2".parse().unwrap();
        assert!(l.allow(a, t) && l.allow(a, t) && !l.allow(a, t));
        for i in 3..9u8 {
            assert!(l.allow(IpAddr::from([192, 168, 1, i]), t));
        }
        assert!(!l.allow(IpAddr::from([192, 168, 1, 99]), t));
        assert!(l.allow(a, t + Duration::from_secs(1)));
    }

    #[test]
    fn empty_sweeps_back_off_to_a_limit() {
        assert_eq!(sweep_pause(0), Duration::from_secs(2));
        assert_eq!(sweep_pause(3), Duration::from_secs(16));
        assert_eq!(sweep_pause(50), Duration::from_secs(256));
    }

    #[test]
    fn found_computers_are_capped_and_their_ids_and_names_clean() {
        let from: SocketAddr = "192.0.2.9:24811".parse().unwrap();
        let answer = |id: &str, name: &str| json!({"app": MAGIC, "id": id, "name": name, "role": "server"});
        let mut found = HashMap::new();
        assert!(!note(&mut found, answer("bad|id\nrow", "x"), from, "me"));
        assert!(!note(&mut found, answer("me", "x"), from, "me"));
        assert!(note(&mut found, answer("desk1", "<b>Desk</b>|x"), from, "me"));
        assert_eq!(found["desk1"].0["name"], "bDesk/bx");
        assert_eq!(found["desk1"].0["host"], "192.0.2.9");
        for i in 0..100 {
            note(&mut found, answer(&format!("pc{i}"), "x"), from, "me");
        }
        assert_eq!(found.len(), MOST_FOUND);
        assert!(note(&mut found, answer("desk1", "Desk"), from, "me"));
    }

    #[test]
    fn only_near_sources_are_answered() {
        assert!(may_answer("192.168.1.5:9".parse().unwrap()));
        assert!(may_answer("127.0.0.1:9".parse().unwrap()));
        assert!(!may_answer("203.0.113.7:9".parse().unwrap()));
    }

    #[test]
    fn machine_id_is_short_hex() {
        let id = machine_id();
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn questions_are_told_apart_from_answers() {
        assert!(is_question(&json!({"app": "lmflow", "q": 1})));
        assert!(!is_question(&json!({"app": "lmflow", "id": "x"})));
        assert!(!is_question(&json!({"app": "other", "q": 1})));
    }

    #[test]
    fn the_sweep_covers_small_private_networks_except_this_computer() {
        let ip = |s: &str| s.parse::<Ipv4Addr>().unwrap();
        let mask = ip("255.255.255.0");
        let list = sweep_of(&[(ip("192.168.1.5"), mask, None)]);
        assert_eq!(list.len(), 253);
        assert!(!list.contains(&ip("192.168.1.5")) && !list.contains(&ip("192.168.1.0")) && !list.contains(&ip("192.168.1.255")));
        assert!(sweep_of(&[(ip("8.8.8.8"), mask, None)]).is_empty());
        assert_eq!(sweep_of(&[(ip("10.0.0.5"), ip("255.0.0.0"), None)]).len(), 253);
    }
}
