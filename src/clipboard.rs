//! Sharing the clipboard through the desktop's wlr data-control protocol.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use wayland_client::backend::ObjectId;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as device, zwlr_data_control_manager_v1 as manager,
    zwlr_data_control_offer_v1 as offer, zwlr_data_control_source_v1 as source,
};

use crate::config::key;
use crate::files::{Log, Sender as FileSender};
use crate::link::Urgency;
use crate::proto;

pub const MAX_BYTES: usize = 32 * 1024 * 1024;

const TEXT_TYPES: [&str; 5] = ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "TEXT"];
const PNG: &str = "image/png";
const URI_LIST: &str = "text/uri-list";
const GNOME_FILES: &str = "x-special/gnome-copied-files";
const SECRET_HINT: &str = "x-kde-passwordManagerHint";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Copied {
    Text(String),
    Picture(Vec<u8>),
    Files(Vec<String>),
}

pub fn announce(item: Copied, log: &Log, share_files: bool, sender: &FileSender, send: impl Fn(Vec<u8>, Urgency)) {
    match item {
        Copied::Text(t) => {
            log(format!("clipboard: sending {} characters", t.chars().count()));
            send(proto::pack(proto::CLIPBOARD, t.as_bytes()), Urgency::Needed);
        }
        Copied::Picture(p) => {
            log(format!("clipboard: sending a picture of {} KB", p.len() / 1024));
            send(proto::pack(proto::PICTURE, &p), Urgency::Spare);
        }
        Copied::Files(f) => {
            log(format!("clipboard: {} copied, {}", if f.len() == 1 { "a file was".to_string() } else { format!("{} files were", f.len()) },
                if share_files { "sending" } else { "not sent: sharing files is off" }));
            if share_files {
                sender.send_copied(&f);
            }
        }
    }
}

pub struct Watcher {
    started: Mutex<Option<Arc<Clipboard>>>,
    out: Sender<Copied>,
    log: Log,
}

impl Watcher {
    pub fn new(out: Sender<Copied>, log: &Log) -> Watcher {
        Watcher { started: Mutex::new(None), out, log: log.clone() }
    }

    pub fn follow(&self, cfg: &crate::config::Config) {
        let Ok(mut started) = self.started.lock() else { return };
        if !cfg.bool(key::SHARE_CLIPBOARD) {
            if let Some(c) = started.take() {
                c.stop();
                (self.log)("clipboard: sharing is off, so nothing copied is read".into());
            }
        } else if started.is_none() {
            match Clipboard::start(self.out.clone(), self.log.clone()) {
                Ok(c) => *started = Some(Arc::new(c)),
                Err(e) => (self.log)(format!("clipboard: cannot start ({e})")),
            }
        }
    }

    pub fn get(&self) -> Option<Arc<Clipboard>> {
        self.started.lock().ok()?.clone()
    }
}

pub fn announce_each<T: Send + Sync + 'static>(
    copied: Receiver<Copied>,
    me: std::sync::Weak<T>,
    sender: Arc<FileSender>,
    facts: impl Fn(&T) -> Option<(Log, bool)> + Send + 'static,
    send: impl Fn(&T, Vec<u8>, Urgency) + Send + 'static,
) {
    std::thread::spawn(move || {
        for item in copied {
            let Some(me) = me.upgrade() else { break };
            let Some((log, share_files)) = facts(&me) else { continue };
            announce(item, &log, share_files, &sender, |m, urgency| send(&me, m, urgency));
        }
    });
}

pub fn facts(cfg: &crate::config::Config, log: &Log) -> Option<(Log, bool)> {
    cfg.bool(key::SHARE_CLIPBOARD).then(|| (log.clone(), cfg.bool(key::SHARE_FILES)))
}

pub fn landed(clipboard: Option<&Clipboard>, log: &Log, item: Copied) {
    match &item {
        Copied::Text(t) => log(format!("clipboard: {} characters arrived", t.chars().count())),
        Copied::Picture(p) => log(format!("clipboard: a picture of {} KB arrived", p.len() / 1024)),
        Copied::Files(_) => {}
    }
    if let Some(c) = clipboard {
        c.apply(item);
    }
}

enum Msg {
    Apply(Copied),
    Got(Copied, bool),
    Note(String),
    Stop,
}

pub fn paths_from_uris(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.strip_prefix("file://").map_or_else(|| l.to_string(), |r| unquote(&r[r.find('/').unwrap_or(0)..])))
        .collect()
}

pub fn uris_from_paths(paths: &[String]) -> String {
    use std::fmt::Write;
    paths.iter().fold(String::new(), |mut s, p| {
        let _ = write!(s, "file://{}\r\n", quote(p));
        s
    })
}

fn quote(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn unquote(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = b.get(i + 1..i + 3).filter(|h| h.iter().all(u8::is_ascii_hexdigit)).and_then(|h| std::str::from_utf8(h).ok());
        match (b[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn choose(types: &[String]) -> Option<&'static str> {
    let has = |t: &str| types.iter().any(|x| x == t);
    if has(SECRET_HINT) {
        return None;
    }
    if has(URI_LIST) {
        return Some(URI_LIST);
    }
    let text = TEXT_TYPES.into_iter().find(|t| has(t));
    if text.is_none() && has(PNG) {
        return Some(PNG);
    }
    text
}

fn decode(mime: &str, data: Vec<u8>) -> Option<Copied> {
    match mime {
        PNG => Some(Copied::Picture(data)),
        URI_LIST => {
            let paths = paths_from_uris(&String::from_utf8_lossy(&data));
            (!paths.is_empty()).then_some(Copied::Files(paths))
        }
        _ => {
            let text = String::from_utf8(data).ok()?;
            (!text.is_empty()).then_some(Copied::Text(text))
        }
    }
}

fn offered(item: &Copied) -> Vec<(&'static str, Arc<[u8]>)> {
    match item {
        Copied::Text(t) => {
            let data: Arc<[u8]> = t.as_bytes().into();
            TEXT_TYPES.into_iter().map(|m| (m, data.clone())).collect()
        }
        Copied::Picture(p) => vec![(PNG, p.as_slice().into())],
        Copied::Files(paths) => {
            let gnome: String = std::iter::once("copy".to_string())
                .chain(paths.iter().map(|p| format!("file://{}", quote(p))))
                .collect::<Vec<_>>()
                .join("\n");
            vec![(URI_LIST, uris_from_paths(paths).into_bytes().into()), (GNOME_FILES, gnome.into_bytes().into())]
        }
    }
}

#[derive(Default, PartialEq)]
struct Last {
    text: Option<String>,
    picture: Option<[u8; 32]>,
    files: Option<Vec<String>>,
}

impl Last {
    fn of(item: &Copied) -> Self {
        let mut last = Self::default();
        match item {
            Copied::Text(t) => last.text = Some(t.clone()),
            Copied::Picture(p) => last.picture = Some(crate::tls::sha256(p)),
            Copied::Files(f) => last.files = Some(f.clone()),
        }
        last
    }

    fn arrived(&mut self, item: &Copied) {
        *self = Self::of(item);
    }

    fn news(&mut self, item: &Copied) -> bool {
        if *self == Self::of(item) {
            return false;
        }
        *self = Self::default();
        true
    }
}

pub struct Clipboard {
    tx: Sender<Msg>,
    wake: UnixStream,
    pub route: Arc<Mutex<String>>,
}

impl Clipboard {
    pub fn start(out: Sender<Copied>, log: Arc<dyn Fn(String) + Send + Sync>) -> std::io::Result<Clipboard> {
        let (tx, rx) = mpsc::channel();
        let (wake, woken) = UnixStream::pair()?;
        woken.set_nonblocking(true)?;
        wake.set_nonblocking(true)?;
        let route = Arc::new(Mutex::new("not started".to_string()));
        let worker = Worker { rx, tx: tx.clone(), wake: wake.try_clone()?, woken, out, log, route: route.clone() };
        std::thread::Builder::new().name("clipboard".into()).spawn(move || worker.run())?;
        Ok(Clipboard { tx, wake, route })
    }

    pub fn apply(&self, item: Copied) {
        let too_big = match &item {
            Copied::Text(t) => t.len() > MAX_BYTES,
            Copied::Picture(p) => p.is_empty() || p.len() > MAX_BYTES,
            Copied::Files(f) => f.is_empty(),
        };
        if !too_big {
            let _ = self.tx.send(Msg::Apply(item));
            let _ = (&self.wake).write(&[1]);
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        self.stop();
    }
}

impl Clipboard {
    pub fn stop(&self) {
        let _ = self.tx.send(Msg::Stop);
        let _ = (&self.wake).write(&[1]);
    }
}

struct Worker {
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    wake: UnixStream,
    woken: UnixStream,
    out: Sender<Copied>,
    log: Arc<dyn Fn(String) + Send + Sync>,
    route: Arc<Mutex<String>>,
}

enum Ended {
    Stopped,
    Lost(String),
}

impl Worker {
    fn set_route(&self, text: &str) {
        if let Ok(mut r) = self.route.lock() {
            *r = text.to_string();
        }
    }

    fn run(self) {
        let mut last = Last::default();
        let mut quick_failures = 0u32;
        loop {
            let started = std::time::Instant::now();
            let ended = match connect() {
                Ok(conn) => self.watch(conn, &mut last),
                Err(e) => Ended::Lost(e),
            };
            match ended {
                Ended::Stopped => return,
                Ended::Lost(why) => {
                    if started.elapsed() > Duration::from_secs(30) {
                        quick_failures = 0;
                    }
                    quick_failures += 1;
                    if quick_failures == 1 || quick_failures % 20 == 0 {
                        (self.log)(format!("clipboard: {why}"));
                    }
                    self.set_route("waiting for the desktop");
                }
            }
            let wait = Duration::from_secs(u64::from(quick_failures.min(10)) * 2);
            if self.wait_for_stop(wait) {
                return;
            }
        }
    }

    fn wait_for_stop(&self, wait: Duration) -> bool {
        let deadline = std::time::Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(Msg::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => return false,
            }
        }
    }

    fn watch(&self, conn: Connection, last: &mut Last) -> Ended {
        let (globals, mut queue): (_, EventQueue<State>) = match registry_queue_init(&conn) {
            Ok(g) => g,
            Err(e) => return Ended::Lost(format!("the desktop did not answer ({e})")),
        };
        let qh = queue.handle();
        let Ok(mgr) = globals.bind::<manager::ZwlrDataControlManagerV1, _, _>(&qh, 1..=1, ()) else {
            return Ended::Lost("this desktop does not share its clipboard (no data-control)".into());
        };
        let Ok(seat) = globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=1, ()) else {
            return Ended::Lost("this desktop has no seat".into());
        };
        let dev = mgr.get_data_device(&seat, &qh, ());
        let mut state = State { tx: Some(self.tx.clone()), wake: self.wake.try_clone().ok(), ..State::default() };
        self.set_route("from the desktop itself (data-control)");
        (self.log)("clipboard: watching the desktop's clipboard".into());

        loop {
            if let Err(e) = queue.dispatch_pending(&mut state) {
                return Ended::Lost(format!("the desktop connection broke ({e})"));
            }
            if state.finished {
                return Ended::Lost("the desktop took the clipboard away".into());
            }
            if let Err(e) = queue.flush() {
                return Ended::Lost(format!("the desktop connection broke ({e})"));
            }
            if let Err(ended) = self.wait_on(&queue) {
                return ended;
            }
            while let Ok(msg) = self.rx.try_recv() {
                if let Some(ended) = self.handle(msg, last, &mgr, &qh, &dev) {
                    return ended;
                }
            }
        }
    }

    fn wait_on(&self, queue: &EventQueue<State>) -> Result<(), Ended> {
        let Some(guard) = queue.prepare_read() else { return Ok(()) };
        let wl = guard.connection_fd().as_raw_fd();
        let mut fds = [
            libc::pollfd { fd: wl, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: self.woken.as_raw_fd(), events: libc::POLLIN, revents: 0 },
        ];
        // SAFETY: fds is a live array of two initialised pollfd structs and
        // its length is passed alongside; poll only writes revents.
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if rc < 0 {
            drop(guard);
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                return Ok(());
            }
            return Err(Ended::Lost("waiting on the desktop failed".into()));
        }
        if fds[0].revents != 0 {
            guard.read().map_err(|e| Ended::Lost(format!("the desktop connection broke ({e})")))?;
        } else {
            drop(guard);
        }
        let mut sink = [0u8; 64];
        while matches!((&self.woken).read(&mut sink), Ok(n) if n > 0) {}
        Ok(())
    }

    fn handle(
        &self,
        msg: Msg,
        last: &mut Last,
        mgr: &manager::ZwlrDataControlManagerV1,
        qh: &QueueHandle<State>,
        dev: &device::ZwlrDataControlDeviceV1,
    ) -> Option<Ended> {
        match msg {
            Msg::Stop => return Some(Ended::Stopped),
            Msg::Note(text) => (self.log)(text),
            Msg::Got(item, seed) => {
                if !seed && last.news(&item) {
                    let _ = self.out.send(item);
                }
            }
            Msg::Apply(item) => {
                last.arrived(&item);
                let list = offered(&item);
                let mimes: Vec<&str> = list.iter().map(|(m, _)| *m).collect();
                let src = mgr.create_data_source(qh, Offered(list));
                for mime in mimes {
                    src.offer(mime.to_string());
                }
                dev.set_selection(Some(&src));
            }
        }
        None
    }
}

fn user_environment(limit: Duration) -> Option<String> {
    let mut child = Command::new("/usr/bin/systemctl")
        .args(["--user", "show-environment"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = tx.send(out.read_to_string(&mut text).map(|_| text));
    });
    let text = rx.recv_timeout(limit).ok().and_then(Result::ok);
    let _ = child.kill();
    let _ = child.wait();
    text
}

fn socket_path() -> Option<PathBuf> {
    let mut display = std::env::var("WAYLAND_DISPLAY").ok().filter(|d| !d.is_empty());
    let mut runtime = std::env::var("XDG_RUNTIME_DIR").ok();
    if display.is_none() {
        if let Some(out) = user_environment(Duration::from_secs(2)) {
            for line in out.lines() {
                match line.split_once('=') {
                    Some(("WAYLAND_DISPLAY", v)) if !v.is_empty() => display = Some(v.to_string()),
                    Some(("XDG_RUNTIME_DIR", v)) if runtime.is_none() => runtime = Some(v.to_string()),
                    _ => {}
                }
            }
        }
    }
    let display = PathBuf::from(display.unwrap_or_else(|| "wayland-0".into()));
    if display.is_absolute() {
        return Some(display);
    }
    runtime.map(|r| PathBuf::from(r).join(display))
}

fn connect() -> Result<Connection, String> {
    let path = socket_path().ok_or("no desktop session to connect to")?;
    let stream = UnixStream::connect(&path).map_err(|e| format!("cannot reach the desktop ({e})"))?;
    Connection::from_socket(stream).map_err(|e| format!("cannot reach the desktop ({e})"))
}

struct Offered(Vec<(&'static str, Arc<[u8]>)>);

#[derive(Default)]
struct State {
    tx: Option<Sender<Msg>>,
    wake: Option<UnixStream>,
    types: HashMap<ObjectId, Vec<String>>,
    current: Option<offer::ZwlrDataControlOfferV1>,
    finished: bool,
    selections: u32,
}

impl State {
    fn note(&self, text: String) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Msg::Note(text));
            if let Some(mut w) = self.wake.as_ref().and_then(|w| w.try_clone().ok()) {
                let _ = w.write(&[1]);
            }
        }
    }

    fn fetch(&mut self, conn: &Connection, of: &offer::ZwlrDataControlOfferV1) {
        let types = self.types.get(&of.id()).cloned().unwrap_or_default();
        let seed = self.selections == 1;
        let Some(mime) = choose(&types) else {
            if !seed && !types.iter().any(|t| t == SECRET_HINT) {
                let kinds: Vec<&str> = types.iter().map(String::as_str).take(12).collect();
                self.note(format!("clipboard: nothing Flow carries was copied (kinds offered: {})", kinds.join(", ")));
            }
            return;
        };
        if READING.swap(true, Ordering::AcqRel) {
            self.note(format!("clipboard: what was copied ({mime}) was skipped, the last copy was still being read"));
            return;
        }
        let Ok((mut reader, writer)) = UnixStream::pair() else {
            READING.store(false, Ordering::Release);
            return;
        };
        of.receive(mime.to_string(), writer.as_fd());
        let _ = conn.flush();
        drop(writer);
        let (tx, wake) = (self.tx.clone(), self.wake.as_ref().and_then(|w| w.try_clone().ok()));
        std::thread::spawn(move || {
            let (got, data) = read_within(&mut reader, READ_LIMIT);
            READING.store(false, Ordering::Release);
            let said = if got.is_err() {
                Some(format!("clipboard: what was copied ({mime}) could not be read from the program offering it"))
            } else if data.len() > MAX_BYTES {
                Some(format!("clipboard: what was copied ({mime}) is over {} MB, too big to carry", MAX_BYTES / (1024 * 1024)))
            } else {
                None
            };
            let Some(tx) = tx else { return };
            let msg = match said {
                Some(text) if !seed => Msg::Note(text),
                Some(_) => return,
                None => match decode(mime, data) {
                    Some(item) => Msg::Got(item, seed),
                    None => return,
                },
            };
            let _ = tx.send(msg);
            if let Some(mut w) = wake {
                let _ = w.write(&[1]);
            }
        });
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<manager::ZwlrDataControlManagerV1, ()> for State {
    fn event(_: &mut Self, _: &manager::ZwlrDataControlManagerV1, _: manager::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<device::ZwlrDataControlDeviceV1, ()> for State {
    fn event(state: &mut Self, _: &device::ZwlrDataControlDeviceV1, event: device::Event, _: &(), conn: &Connection, _: &QueueHandle<Self>) {
        match event {
            device::Event::DataOffer { id } => {
                state.types.insert(id.id(), Vec::new());
            }
            device::Event::Selection { id } => {
                if let Some(old) = state.current.take() {
                    state.types.remove(&old.id());
                    old.destroy();
                }
                state.selections += 1;
                if let Some(of) = id {
                    state.fetch(conn, &of);
                    state.current = Some(of);
                }
            }
            device::Event::PrimarySelection { id: Some(of) } => {
                state.types.remove(&of.id());
                of.destroy();
            }
            device::Event::Finished => state.finished = true,
            _ => {}
        }
    }

    event_created_child!(State, device::ZwlrDataControlDeviceV1, [
        device::EVT_DATA_OFFER_OPCODE => (offer::ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<offer::ZwlrDataControlOfferV1, ()> for State {
    fn event(state: &mut Self, of: &offer::ZwlrDataControlOfferV1, event: offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let offer::Event::Offer { mime_type } = event {
            if let Some(list) = state.types.get_mut(&of.id()) {
                list.push(mime_type);
            }
        }
    }
}

impl Dispatch<source::ZwlrDataControlSourceV1, Offered> for State {
    fn event(_: &mut Self, src: &source::ZwlrDataControlSourceV1, event: source::Event, data: &Offered, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            source::Event::Send { mime_type, fd } => {
                let Some((_, bytes)) = data.0.iter().find(|(m, _)| *m == mime_type) else { return };
                let bytes = bytes.clone();
                std::thread::spawn(move || write_all(fd, &bytes, WRITE_LIMIT));
            }
            source::Event::Cancelled => src.destroy(),
            _ => {}
        }
    }
}

const READ_LIMIT: Duration = Duration::from_secs(10);
const WRITE_LIMIT: Duration = Duration::from_secs(10);

static READING: AtomicBool = AtomicBool::new(false);

fn read_within(reader: &mut UnixStream, limit: Duration) -> (std::io::Result<()>, Vec<u8>) {
    let end = Instant::now() + limit;
    let mut data = Vec::new();
    let mut chunk = [0u8; 65536];
    while data.len() <= MAX_BYTES {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return (Err(std::io::ErrorKind::TimedOut.into()), data);
        }
        let _ = reader.set_read_timeout(Some(left));
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => data.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return (Err(e), data),
        }
    }
    (Ok(()), data)
}

fn write_all(fd: OwnedFd, bytes: &[u8], limit: Duration) {
    let raw = fd.as_raw_fd();
    // SAFETY: raw is the open descriptor `fd` owns for this whole function.
    unsafe { libc::fcntl(raw, libc::F_SETFL, libc::fcntl(raw, libc::F_GETFL) | libc::O_NONBLOCK) };
    let mut file = std::fs::File::from(fd);
    let end = Instant::now() + limit;
    let mut sent = 0;
    while sent < bytes.len() {
        match file.write(&bytes[sent..]) {
            Ok(0) => return,
            Ok(n) => sent += n,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return;
                }
                let mut p = libc::pollfd { fd: raw, events: libc::POLLOUT, revents: 0 };
                // SAFETY: p is one valid pollfd and the count given is 1.
                unsafe { libc::poll(&mut p, 1, left.as_millis().min(i32::MAX as u128) as i32) };
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_copied_is_sent_once_sharing_the_clipboard_is_off() {
        let log: Log = Arc::new(|_| {});
        let cfg = |clip: bool, files: bool| {
            crate::config::Config(serde_json::json!({ key::SHARE_CLIPBOARD: clip, key::SHARE_FILES: files }).as_object().unwrap().clone())
        };
        assert!(facts(&cfg(false, true), &log).is_none());
        assert_eq!(facts(&cfg(true, false), &log).map(|(_, files)| files), Some(false));
        assert_eq!(facts(&cfg(true, true), &log).map(|(_, files)| files), Some(true));
    }

    #[test]
    fn switching_sharing_off_stops_the_watcher() {
        let home = crate::config::tests::TestHome::new();
        let before = std::env::var_os("WAYLAND_DISPLAY");
        // SAFETY: DIR_LOCK is held, so no other test reads the environment now.
        unsafe { std::env::set_var("WAYLAND_DISPLAY", home.dir.join("no-desktop")) };
        let log: Log = Arc::new(|_| {});
        let watcher = Watcher::new(mpsc::channel().0, &log);
        let share = |on: bool| crate::config::Config(serde_json::json!({ key::SHARE_CLIPBOARD: on }).as_object().unwrap().clone());
        watcher.follow(&share(true));
        assert!(watcher.get().is_some());
        watcher.follow(&share(false));
        assert!(watcher.get().is_none());
        watcher.follow(&share(true));
        assert!(watcher.get().is_some());
        // SAFETY: as above.
        unsafe {
            match before {
                Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
                None => std::env::remove_var("WAYLAND_DISPLAY"),
            }
        }
    }

    #[test]
    fn nothing_is_watched_while_sharing_the_clipboard_is_off() {
        let log: Log = Arc::new(|_| {});
        let watcher = Watcher::new(mpsc::channel().0, &log);
        assert!(watcher.get().is_none());
        let off = crate::config::Config(serde_json::json!({ key::SHARE_CLIPBOARD: false }).as_object().unwrap().clone());
        watcher.follow(&off);
        assert!(watcher.get().is_none());
    }

    #[test]
    fn a_writer_to_a_reader_that_never_reads_gives_up() {
        let (a, _b) = UnixStream::pair().unwrap();
        let started = Instant::now();
        write_all(OwnedFd::from(a), &vec![0u8; 8 * 1024 * 1024], Duration::from_millis(200));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_reader_of_a_program_that_never_finishes_gives_up() {
        let (mut a, _b) = UnixStream::pair().unwrap();
        let started = Instant::now();
        let (got, data) = read_within(&mut a, Duration::from_millis(200));
        assert!(got.is_err() && data.is_empty());
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
