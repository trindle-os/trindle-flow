//! The computer whose mouse and keyboard control the other one.

use std::net::TcpListener;
use std::os::fd::{AsFd, AsRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::capture::{Captures, Said, Side};
use crate::clipboard::{self, Copied};
use crate::config::{self, key, Config};
use crate::discovery::{self, clean_id, Responder};
use crate::files::{self, Log};
use crate::layout::Layout;
use crate::link::{Link, Urgency};
use crate::proto::{self, Event, EV_KEY, EV_REL, REL_HWHEEL, REL_WHEEL};
use crate::unlock::{self, Answer};
use crate::{page, status, tls};

mod away;
mod greet;

use away::Away;
use greet::accept_loop;

const HEARTBEAT: Duration = Duration::from_secs(2);
const SILENCE: Duration = Duration::from_secs(8);
const REARM: Duration = Duration::from_secs(10);
const ASKING_AT_ONCE: usize = 8;
const TICK: Duration = Duration::from_secs(1);
const ASKING_KEPT: Duration = Duration::from_secs(60);
const UNLOCK_ANSWER: Duration = Duration::from_secs(12);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn clean_name(raw: &str, fallback: &str) -> String {
    let name = page::plain(raw, 40);
    let name = name.trim();
    if name.is_empty() { fallback.to_string() } else { name.to_string() }
}

#[derive(Clone)]
struct Peer {
    id: String,
    name: String,
    layout: Layout,
    link: Arc<Link>,
}

enum Wake {
    Said((Side, Said)),
    Closed(String, String),
    Asked,
    Lost,
}

struct Shared {
    cfg: RwLock<Config>,
    log: Log,
    peers: Mutex<Vec<Peer>>,
    receivers: Mutex<std::collections::HashMap<String, files::Receiver>>,
    clipboard: clipboard::Watcher,
    wake: Sender<Wake>,
    relay: SyncSender<(String, u8, Vec<u8>)>,
    file_turn: FileTurn,
    asking: Mutex<Vec<(Instant, Value)>>,
    unlocking: Mutex<Option<(String, Sender<Answer>)>>,
}

impl Shared {
    fn asking_now(&self) -> Vec<Value> {
        let mut list = lock(&self.asking);
        list.retain(|(when, _)| when.elapsed() < ASKING_KEPT);
        list.iter().map(|(_, v)| v.clone()).collect()
    }

    fn asked(&self, id: &str, name: &str, loud: &dyn Fn() -> bool) {
        let new = {
            let mut list = lock(&self.asking);
            let asked = list.iter().find(|(_, v)| v["id"] == id).and_then(|(_, v)| v["asked"].as_u64());
            let new = asked.is_none();
            let entry = json!({"id": id, "name": name, "asked": asked.unwrap_or(config::now_secs() % 1_000_000)});
            list.retain(|(when, v)| v["id"] != id && when.elapsed() < ASKING_KEPT);
            if list.len() >= ASKING_AT_ONCE {
                list.remove(0);
            }
            list.push((Instant::now(), entry));
            new
        };
        let mut state = status::read();
        state[status::key::ASKING] = json!(self.asking_now());
        status::write(&state);
        if new && loud() {
            (self.log)(format!("{name} asks to pair"));
            page::tell(
                "Flow",
                &format!("{name} asks to be controlled by this computer. To allow it, pick it in Flow's settings, under Computer asking to be controlled, and type the number it shows."),
            );
        }
    }

    fn cfg(&self) -> std::sync::RwLockReadGuard<'_, Config> {
        self.cfg.read().unwrap_or_else(|e| e.into_inner())
    }

    fn to_everyone(&self, blob: &[u8], skip: Option<&str>, urgency: Urgency) -> bool {
        let targets: Vec<Arc<Link>> =
            lock(&self.peers).iter().filter(|p| Some(p.id.as_str()) != skip).map(|p| p.link.clone()).collect();
        let mut sent = false;
        for link in targets {
            sent |= link.send(blob.to_vec(), urgency);
        }
        sent
    }

    fn unlock(&self, id: &str, pin: unlock::Pin) -> Answer {
        let peer = if id.is_empty() {
            self.newest()
        } else {
            lock(&self.peers).iter().find(|p| p.id == id && p.link.alive()).cloned()
        };
        let Some(peer) = peer else { return Answer::no("The other computer is not connected.") };
        let (tx, rx) = mpsc::channel();
        *lock(&self.unlocking) = Some((peer.id.clone(), tx));
        let sent = peer.link.send(proto::pack(proto::UNLOCK, pin.as_str().as_bytes()), Urgency::Needed);
        drop(pin);
        let answer = if sent {
            rx.recv_timeout(UNLOCK_ANSWER).unwrap_or_else(|_| Answer::no("The other computer did not answer in time."))
        } else {
            Answer::no("The other computer is not connected.")
        };
        *lock(&self.unlocking) = None;
        answer
    }

    fn newest(&self) -> Option<Peer> {
        lock(&self.peers).iter().rev().find(|p| p.link.alive()).cloned()
    }
}

pub fn run(running: Arc<AtomicBool>, log: Log) -> Result<(), crate::Stop> {
    page::take_and_tell(&*log);
    let cfg = config::load();
    let tls_cfg = tls::server_config()?;
    let port = u16::try_from(cfg.i64(key::PORT)).map_err(|_| "the port is not a number from 1 to 65535".to_string())?;
    let listener = TcpListener::bind(("0.0.0.0", port)).map_err(|e| format!("cannot listen on port {port} ({e})"))?;
    files::clear_temporary();

    let (copied_tx, copied_rx) = mpsc::channel();
    let clipboard = clipboard::Watcher::new(copied_tx, &log);
    clipboard.follow(&cfg);
    let (wake_tx, wake_rx) = mpsc::channel();
    let (relay_tx, relay_rx) = mpsc::sync_channel::<(String, u8, Vec<u8>)>(32);
    let me = Arc::new(Shared {
        cfg: RwLock::new(cfg.clone()),
        log: log.clone(),
        peers: Mutex::new(Vec::new()),
        receivers: Mutex::new(std::collections::HashMap::new()),
        clipboard,
        wake: wake_tx.clone(),
        relay: relay_tx,
        file_turn: FileTurn::default(),
        asking: Mutex::new(Vec::new()),
        unlocking: Mutex::new(None),
    });
    let unlocking = {
        let me = me.clone();
        unlock::listen(move |id, pin| me.unlock(id, pin), log.clone())
    };
    let asked = watch_asked(wake_tx.clone());

    pass_on(&me, copied_rx, relay_rx);
    let accepting = {
        let (me, running) = (me.clone(), running.clone());
        crate::Stopping::spawn(move |stop| accept_loop(me, listener, tls_cfg, running, stop))
    };
    let responder = cfg.bool(key::DISCOVERY).then(|| answer_seekers(&me));

    log(format!("Flow {}: sharing this computer's mouse and keyboard, listening on port {port}", env!("CARGO_PKG_VERSION")));

    page::write_computers(&cfg);
    let mut server = Server::new(me.clone(), cfg, wake_rx, wake_tx);
    let result = server.run_loop(&running);
    server.go_home(true);
    for p in lock(&me.peers).drain(..) {
        p.link.close();
    }
    drop((responder, accepting, unlocking, asked, server));
    if let Some(c) = me.clipboard.get() {
        c.stop();
    }
    status::clear();
    result
}

#[derive(Default)]
struct FileTurn(Mutex<Option<String>>);

impl FileTurn {
    fn may_pass(&self, from: &str, kind: u8, body: &[u8]) -> bool {
        let mut turn = lock(&self.0);
        if kind == proto::FILE_BEGIN && serde_json::from_slice::<Value>(body).is_ok_and(|h| h["first"] == true) {
            *turn = Some(from.to_string());
        }
        turn.as_deref() == Some(from)
    }
}

fn pass_on(me: &Arc<Shared>, copied: Receiver<Copied>, relay: Receiver<(String, u8, Vec<u8>)>) {
    let weak = Arc::downgrade(me);
    let sender = Arc::new(files::Sender::new(
        {
            let me = weak.clone();
            Arc::new(move |kind, body| me.upgrade().is_some_and(|me| me.file_turn.may_pass("", kind, &body) && me.to_everyone(&proto::pack(kind, &body), None, Urgency::Wait)))
        },
        me.log.clone(),
    ));
    clipboard::announce_each(
        copied,
        weak.clone(),
        sender,
        |me| clipboard::facts(&me.cfg(), &me.log),
        |me, m, urgency| {
            me.to_everyone(&m, None, urgency);
        },
    );
    std::thread::spawn(move || {
        for (from, kind, body) in relay {
            let Some(me) = weak.upgrade() else { break };
            if me.file_turn.may_pass(&from, kind, &body) {
                me.to_everyone(&proto::pack(kind, &body), Some(&from), Urgency::Wait);
            }
        }
    });
}

fn answer_seekers(me: &Arc<Shared>) -> Responder {
    let (me, log) = (me.clone(), me.log.clone());
    Responder::start(
        move || {
            json!({
                "app": discovery::MAGIC, "v": 1, "role": "server",
                "id": discovery::machine_id(), "name": discovery::machine_name(),
                "port": me.cfg().i64(key::PORT),
                "connected": lock(&me.peers).len(),
            })
        },
        move |m| log(m),
    )
}

type Stamps = (Option<config::Stamp>, Option<config::Stamp>);

struct Server {
    me: Arc<Shared>,
    cfg: Config,
    captures: Option<Captures>,
    refused_sides: std::collections::HashMap<Side, Instant>,
    active_side: Option<Side>,
    mode: ModeSent,
    stray_said: bool,
    rearm_at: Instant,
    refused_said: Option<String>,
    away: Option<Away>,
    active: Option<Peer>,
    wake: Receiver<Wake>,
    waker: Sender<Wake>,
    started: Instant,
    last_ping: Option<Instant>,
    files_seen: Option<(Stamps, Instant)>,
}

impl Server {
    fn new(me: Arc<Shared>, cfg: Config, wake: Receiver<Wake>, waker: Sender<Wake>) -> Server {
        Server {
            me,
            cfg,
            captures: None,
            refused_sides: std::collections::HashMap::new(),
            active_side: None,
            mode: ModeSent::default(),
            stray_said: false,
            rearm_at: Instant::now(),
            refused_said: None,
            away: None,
            active: None,
            wake,
            waker,
            started: Instant::now(),
            last_ping: None,
            files_seen: None,
        }
    }

    fn log(&self, text: impl Into<String>) {
        (self.me.log)(text.into());
    }

    fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn side(&self) -> Side {
        Side::parse(&self.cfg.str(key::OTHER_SIDE)).unwrap_or(Side::Right)
    }

    fn wanted(&self) -> Vec<(Side, Option<String>)> {
        let chosen: Vec<(Side, Option<String>)> = Side::ALL
            .iter()
            .filter_map(|&side| {
                let id = self.cfg.str(&format!("{}_computer", side.name()));
                (!id.is_empty()).then_some((side, Some(id)))
            })
            .collect();
        if chosen.is_empty() {
            vec![(self.side(), None)]
        } else {
            chosen
        }
    }

    fn run_loop(&mut self, running: &AtomicBool) -> Result<(), crate::Stop> {
        let mut next_tick = Instant::now();
        while running.load(Ordering::Relaxed) {
            // While the pointer is away every movement wakes this loop, so arming waits for the tick then.
            if self.away.is_none() || Instant::now() >= next_tick {
                self.arm();
            }
            match self.wake.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
                Ok(first) => {
                    self.woke(first);
                    while let Ok(more) = self.wake.try_recv() {
                        self.woke(more);
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {}
            }
            if self.captures.as_ref().is_some_and(Captures::lost) {
                return Err(crate::Stop::Gone);
            }
            self.watch_the_peer();
            if Instant::now() >= next_tick {
                next_tick = Instant::now() + TICK;
                self.do_asked();
                if self.reload_config() {
                    return Ok(());
                }
                self.publish();
            }
        }
        Ok(())
    }

    fn woke(&mut self, wake: Wake) {
        match wake {
            Wake::Said(said) => self.heard(said),
            Wake::Closed(id, why) => self.drop_peer(&id, &why),
            Wake::Asked => self.do_asked(),
            Wake::Lost => {}
        }
    }

    fn do_asked(&mut self) {
        let path = config::path("go.json");
        let Ok(text) = std::fs::read_to_string(&path) else { return };
        let _ = std::fs::remove_file(&path);
        let note: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if note["at"].as_u64().is_none_or(|at| config::now_secs().saturating_sub(at) > 5) {
            return;
        }
        match note["do"].as_str().unwrap_or("") {
            "home" => self.go_home(true),
            "goto" => self.take_for(note["id"].as_str().unwrap_or("")),
            "switch" => {
                if self.active.is_some() {
                    self.go_home(true);
                } else {
                    self.take_for("");
                }
            }
            _ => {}
        }
    }

    fn take_for(&mut self, id: &str) {
        if self.active.is_some() {
            self.go_home(true);
        }
        let live: Vec<String> = lock(&self.me.peers).iter().filter(|p| p.link.alive()).map(|p| p.id.clone()).collect();
        let newest = self.me.newest().map(|p| p.id);
        let target = self.wanted().into_iter().find(|(_, chosen)| {
            let who = chosen.clone().or_else(|| newest.clone()).unwrap_or_default();
            live.contains(&who) && (id.is_empty() || who == id)
        });
        let Some((side, _)) = target else {
            self.log("nobody connected to go to");
            return;
        };
        if !self.captures.as_ref().is_some_and(|c| c.take(side)) {
            self.log("this desktop cannot hand over the mouse and keyboard on request: update Trindle");
        }
    }

    fn arm(&mut self) {
        let now = Instant::now();
        if self.captures.is_none() {
            if now < self.rearm_at {
                return;
            }
            self.rearm_at = now + REARM;
            let waker = self.waker.clone();
            match Captures::start(Box::new(move |said| {
                let _ = waker.send(said.map_or(Wake::Lost, Wake::Said));
            })) {
                Ok(c) => {
                    self.captures = Some(c);
                    self.mode.let_go();
                }
                Err(e) => {
                    self.refused(e);
                    return;
                }
            }
        }
        let wanted: Vec<Side> = self.wanted().into_iter().map(|(s, _)| s).collect();
        let armed = self.captures.as_ref().map(Captures::armed_sides).unwrap_or_default();
        for side in armed.into_iter().filter(|s| !wanted.contains(s)) {
            if self.active_side == Some(side) {
                self.go_home(true);
            }
            if let Some(c) = self.captures.as_mut() {
                c.disarm(side);
            }
        }
        if self.captures.as_ref().is_some_and(|c| c.armed_sides().is_empty()) {
            self.mode.let_go();
        }
        self.refused_sides.retain(|s, _| wanted.contains(s));
        for side in wanted {
            let Some(c) = self.captures.as_mut() else { return };
            if c.armed(side) || self.refused_sides.get(&side).is_some_and(|t| now < *t) {
                continue;
            }
            c.arm(side);
            self.log(format!("asked for the pointer to cross {}", side.said_screen()));
        }
        let manual = self.cfg.bool(key::SHORTCUT_ONLY);
        if let Some(c) = &self.captures {
            if self.mode.due(manual, !c.armed_sides().is_empty()) {
                c.set_manual(manual);
            }
        }
    }

    fn side_refused(&mut self, side: Side, why: String) {
        if let Some(c) = self.captures.as_mut() {
            c.disarm(side);
        }
        self.refused_sides.insert(side, Instant::now() + REARM);
        self.refused(why);
    }

    fn refused(&mut self, why: String) {
        if self.refused_said.as_deref() != Some(why.as_str()) {
            self.log(why.clone());
            page::tell("Flow cannot share your mouse and keyboard", &why);
            self.refused_said = Some(why);
        }
    }

    fn heard(&mut self, (side, said): (Side, Said)) {
        let from_active = self.active_side == Some(side);
        match said {
            Said::Ready => {
                self.refused_said = None;
                self.log("ready: push the pointer off that side of the screen to cross");
            }
            Said::Taken => {
                self.side_refused(side, "Another program is sharing your mouse and keyboard, so Flow is waiting.".into());
            }
            Said::Activated(along) => self.cross(side, along),
            Said::NotNow => self.log("the desktop cannot hand over the mouse and keyboard just now"),
            Said::NotAsked => self.log(
                "the desktop hands Flow the mouse and keyboard only right after you use Flow's shortcut or menu item",
            ),
            Said::Key { .. } if !from_active => {
                if !self.stray_said {
                    self.stray_said = true;
                    self.log("the desktop handed Flow a key while the pointer is on this computer; it was not passed on");
                }
            }
            _ if !from_active => {}
            Said::Deactivated { by_flow } => {
                if !by_flow && self.active.is_some() {
                    self.log("the desktop took the mouse and keyboard back");
                }
                self.go_home(false);
            }
            Said::Motion { dx, dy } => {
                let now = self.now();
                let Some(away) = self.away.as_mut() else { return };
                let speed = self.cfg.f64(key::POINTER_SPEED).clamp(0.25, 4.0);
                let (events, home) = away.step(dx * speed, dy * speed, now);
                self.send(&events);
                if home {
                    self.log("pushed back against the side facing home");
                    self.go_home(true);
                }
            }
            Said::Button { button, pressed } => self.send(&[(EV_KEY, button as u16, i32::from(pressed))]),
            Said::Key { key, pressed } => self.send(&[(EV_KEY, key as u16, i32::from(pressed))]),
            Said::Axis { axis, value, v120 } => {
                if let Some(e) = self.away.as_mut().and_then(|a| a.scroll(axis, value, v120)) {
                    self.send(&[e]);
                }
            }
        }
    }

    fn send(&mut self, events: &[Event]) {
        if events.is_empty() {
            return;
        }
        let Some(peer) = &self.active else { return };
        if !peer.link.send(proto::pack_events(events), Urgency::Needed) {
            self.log(format!("{} is not keeping up, bringing the pointer back", peer.name));
            self.go_home(true);
        }
    }

    fn cross(&mut self, side: Side, along: f64) {
        let wanted = self.wanted().into_iter().find(|(s, _)| *s == side).and_then(|(_, id)| id);
        let peer = match &wanted {
            Some(id) => lock(&self.me.peers).iter().rev().find(|p| &p.id == id && p.link.alive()).cloned(),
            None => self.me.newest(),
        };
        let Some(peer) = peer else {
            self.log(format!("nobody {} to cross to yet", side.said_screen()));
            if let Some(c) = &self.captures {
                c.release(side, along);
            }
            return;
        };
        let away = Away::new(side, along, peer.layout.clone());
        peer.link.touch();
        peer.link.send(proto::pack_json(proto::ENTER, &json!({"x": away.x as i32, "y": away.y as i32})), Urgency::Needed);
        self.log(format!("pointer moved to {}", peer.name));
        self.away = Some(away);
        self.active = Some(peer);
        self.active_side = Some(side);
        self.stray_said = false;
        self.publish();
    }

    fn go_home(&mut self, release: bool) {
        let away = self.away.take();
        let Some(peer) = self.active.take() else { return };
        peer.link.send(proto::pack(proto::LEAVE, b""), Urgency::Needed);
        let side = self.active_side.take();
        if release {
            if let (Some(c), Some(side)) = (&self.captures, side) {
                c.release(side, away.as_ref().map_or(0.5, Away::along_home));
            }
        }
        self.log(format!("pointer back from {}", peer.name));
        self.publish();
    }

    fn drop_peer(&mut self, id: &str, why: &str) {
        let gone: Vec<Peer> = {
            let mut peers = lock(&self.me.peers);
            let (dead, live): (Vec<Peer>, Vec<Peer>) = peers.drain(..).partition(|p| p.id == id && !p.link.alive());
            *peers = live;
            dead
        };
        for peer in gone {
            if self.active.as_ref().is_some_and(|a| Arc::ptr_eq(&a.link, &peer.link)) {
                self.go_home(true);
            }
            if let Some(mut r) = lock(&self.me.receivers).remove(id) {
                r.forget();
            }
            self.log(format!("{} disconnected ({why})", peer.name));
        }
        self.publish();
    }

    fn watch_the_peer(&mut self) {
        if self.last_ping.is_none_or(|t| t.elapsed() > HEARTBEAT) {
            self.last_ping = Some(Instant::now());
            self.me.to_everyone(&proto::pack(proto::PING, b""), None, Urgency::Needed);
        }
        let Some(peer) = self.active.clone() else { return };
        let why = if !peer.link.alive() {
            "is not keeping up"
        } else if peer.link.quiet_for() > SILENCE {
            "has gone quiet"
        } else {
            return;
        };
        self.log(format!("{} {why}, bringing the pointer back", peer.name));
        self.go_home(true);
        peer.link.close();
    }

    fn reload_config(&mut self) -> bool {
        let stamps = (page::stamp(), config::stamp());
        // Read anyway now and then: a file written twice within the clock's step keeps its time.
        if self.files_seen.as_ref().is_some_and(|(seen, at)| *seen == stamps && at.elapsed() < Duration::from_secs(10)) {
            return false;
        }
        self.files_seen = Some((stamps, Instant::now()));
        page::take_and_tell(|m| self.log(m));
        let fresh = config::load();
        if fresh == self.cfg {
            return false;
        }
        if let Ok(mut c) = self.me.cfg.write() {
            *c = fresh.clone();
        }
        self.me.clipboard.follow(&fresh);
        if fresh.str(key::TOKEN) != self.cfg.str(key::TOKEN) {
            for p in lock(&self.me.peers).drain(..) {
                p.link.close();
            }
            self.go_home(true);
        }
        self.cfg = fresh;
        let drop_id = self.cfg.str(key::DROP_ID);
        if !drop_id.is_empty() {
            let mut dropped: Vec<Value> = self.cfg.0.get(key::DROPPED).and_then(Value::as_array).cloned().unwrap_or_default();
            if !dropped.contains(&json!(drop_id)) {
                dropped.push(json!(drop_id));
            }
            config::set_value(key::DROPPED, json!(dropped));
            self.cfg = config::set_value(key::DROP_ID, json!(""));
            if let Ok(mut c) = self.me.cfg.write() {
                *c = self.cfg.clone();
            }
            let links: Vec<Arc<Link>> = lock(&self.me.peers).iter().filter(|p| p.id == drop_id).map(|p| p.link.clone()).collect();
            for link in links {
                link.close();
            }
        }
        page::write_computers(&self.cfg);
        !self.cfg.controls()
    }

    fn publish(&self) {
        let peer = self.me.newest();
        let connected: Vec<Value> = lock(&self.me.peers)
            .iter()
            .filter(|p| p.link.alive())
            .map(|p| json!({"id": p.id, "name": p.name}))
            .collect();
        status::write(&json!({
            (status::key::ROLE): "control",
            (status::key::RUNNING): true,
            (status::key::CONNECTED): peer.is_some(),
            (status::key::PEERS): connected,
            (status::key::PEER_NAME): peer.as_ref().map(|p| p.name.clone()),
            (status::key::AWAY): self.active.is_some(),
            (status::key::SIDE): self.side().name(),
            (status::key::CAN_CROSS): self.captures.as_ref().is_some_and(|c| !c.armed_sides().is_empty()),
            (status::key::FOUND): [],
            (status::key::ASKING): self.me.asking_now(),
        }));
    }
}

fn watch_asked(wake: Sender<Wake>) -> Option<crate::Stopping> {
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    config::ensure_dir().ok()?;
    let dir = std::ffi::CString::new(config::dir().as_os_str().as_bytes()).ok()?;
    // SAFETY: inotify_init1 takes no pointers; a negative answer is checked.
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    // SAFETY: fd was just made and nothing else owns it.
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    // SAFETY: dir is a NUL-terminated path that outlives the call.
    if unsafe { libc::inotify_add_watch(fd.as_raw_fd(), dir.as_ptr(), libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO) } < 0 {
        return None;
    }
    Some(crate::Stopping::spawn(move |bell| {
        let mut buf = [0u8; 4096];
        while bell.wait_for(fd.as_fd()) {
            // SAFETY: buf is writable for its whole length, which is passed with it.
            while unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) } > 0 {}
            if wake.send(Wake::Asked).is_err() {
                break;
            }
        }
    }))
}

#[derive(Default)]
struct ModeSent(Option<bool>);

impl ModeSent {
    fn let_go(&mut self) {
        self.0 = None;
    }

    fn due(&mut self, manual: bool, holding: bool) -> bool {
        let due = holding && self.0 != Some(manual);
        if due {
            self.0 = Some(manual);
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use greet::arrived;

    pub(in crate::server) fn shared(cfg: Value) -> (Shared, Arc<Mutex<Vec<String>>>) {
        let said = Arc::new(Mutex::new(Vec::new()));
        let log: Log = {
            let said = said.clone();
            Arc::new(move |line| lock(&said).push(line))
        };
        let Value::Object(cfg) = cfg else { unreachable!() };
        let (wake, _) = mpsc::channel();
        let (relay, _) = mpsc::sync_channel(1);
        let me = Shared {
            cfg: RwLock::new(Config(cfg)),
            log: log.clone(),
            peers: Mutex::new(Vec::new()),
            receivers: Mutex::new(std::collections::HashMap::new()),
            clipboard: clipboard::Watcher::new(mpsc::channel().0, &log),
            wake,
            relay,
            file_turn: FileTurn::default(),
            asking: Mutex::new(Vec::new()),
            unlocking: Mutex::new(None),
        };
        (me, said)
    }

    #[test]
    fn a_pong_updates_the_stored_screen_size_and_a_bare_one_changes_nothing() {
        let _home = crate::config::tests::TestHome::new();
        let (me, _) = shared(json!({}));
        let (link, _near) = crate::link::tests::pair();
        lock(&me.peers).push(Peer { id: "desk".into(), name: "Desk".into(), layout: Layout::single(1920, 1080), link: link.clone() });
        let size = || lock(&me.peers)[0].layout.reach();
        arrived(&me, "desk", &link, proto::PONG, Vec::new());
        assert_eq!(size(), (1920, 1080));
        arrived(&me, "desk", &link, proto::PONG, proto::pong(&Layout::single(2560, 1440))[5..].to_vec());
        assert_eq!(size(), (2560, 1440));
    }

    #[test]
    fn only_the_latest_eight_computers_asking_are_kept() {
        let _home = crate::config::tests::TestHome::new();
        let (me, _) = shared(json!({}));
        for n in 0..20 {
            me.asked(&format!("lap{n}"), "Laptop", &|| true);
        }
        me.asked("lap15", "Laptop", &|| true);
        let ids: Vec<String> = me.asking_now().iter().map(|v| v["id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, ["lap12", "lap13", "lap14", "lap16", "lap17", "lap18", "lap19", "lap15"]);
    }

    #[test]
    fn a_run_ending_stops_listening_and_lets_go_of_the_port() {
        let _home = crate::config::tests::TestHome::new();
        let (me, _) = shared(json!({}));
        let me = Arc::new(me);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let tls_cfg = tls::server_config().unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let accepting = {
            let me = me.clone();
            crate::Stopping::spawn(move |stop| accept_loop(me, listener, tls_cfg, running, stop))
        };
        assert!(TcpListener::bind(("127.0.0.1", port)).is_err());
        drop(accepting);
        assert!(TcpListener::bind(("127.0.0.1", port)).is_ok());
        assert_eq!(Arc::strong_count(&me), 1, "the loop let go of what it shared");
    }

    #[test]
    fn a_note_wakes_the_loop_at_once_and_a_rung_bell_stops_the_watcher() {
        let _home = crate::config::tests::TestHome::new();
        let (tx, rx) = mpsc::channel();
        let watching = watch_asked(tx).expect("inotify");
        config::write("go.json", b"{}").unwrap();
        assert!(matches!(rx.recv_timeout(Duration::from_secs(2)), Ok(Wake::Asked)));
        let start = Instant::now();
        drop(watching);
        assert!(start.elapsed() < Duration::from_millis(500), "stopped at once");
    }

    #[test]
    fn a_sleeping_thread_wakes_as_soon_as_its_bell_rings() {
        let start = Instant::now();
        let sleeping = crate::Stopping::spawn(|bell| {
            assert!(!bell.sleep(Duration::from_secs(30)));
        });
        std::thread::sleep(Duration::from_millis(50));
        drop(sleeping);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn scrolling_goes_as_whole_wheel_clicks() {
        let mut a = Away::new(Side::Right, 0.5, Layout::single(1280, 720));
        assert_eq!(a.scroll(0, 15.0, 120), Some((EV_REL, REL_WHEEL, -1)));
        assert_eq!(a.scroll(0, -7.5, 0), None);
        assert_eq!(a.scroll(0, -7.5, 0), Some((EV_REL, REL_WHEEL, 1)));
        assert_eq!(a.scroll(1, 30.0, 0), Some((EV_REL, REL_HWHEEL, 2)));
    }

    #[test]
    fn secrets_and_names_are_checked() {
        assert!(same_secret(b"abc", b"abc"));
        assert!(!same_secret(b"abc", b"abd") && !same_secret(b"abc", b"ab"));
        assert_eq!(clean_id("ab/../c d"), "abcd");
        assert_eq!(clean_name("  \u{7}John's laptop ", "10.0.0.2"), "John's laptop");
        assert_eq!(clean_name("", "10.0.0.2"), "10.0.0.2");
        assert_eq!(clean_name("<b>Desk</b>|x\nforged", "?"), "bDesk/bxforged");
        assert_eq!(clean_name(&"n".repeat(500), "?").len(), 40);
    }

    #[test]
    fn shortcut_only_is_sent_again_after_the_last_capture_goes() {
        let mut mode = ModeSent::default();
        assert!(!mode.due(true, false));
        assert!(mode.due(true, true));
        assert!(!mode.due(true, true));
        mode.let_go();
        assert!(mode.due(true, true));
        assert!(mode.due(false, true));
    }

    #[test]
    fn copied_files_are_passed_on_one_computer_at_a_time_newest_first() {
        let turn = FileTurn::default();
        let begin = |first: bool| format!(r#"{{"id":1,"name":"a","size":1,"first":{first},"last":true}}"#).into_bytes();
        assert!(!turn.may_pass("b", proto::FILE_PIECE, b"\0\0\0\x01x"));
        assert!(turn.may_pass("b", proto::FILE_BEGIN, &begin(true)));
        assert!(turn.may_pass("b", proto::FILE_PIECE, b"\0\0\0\x01x"));
        assert!(turn.may_pass("c", proto::FILE_BEGIN, &begin(true)));
        assert!(!turn.may_pass("b", proto::FILE_PIECE, b"\0\0\0\x01x"));
        assert!(!turn.may_pass("b", proto::FILE_BEGIN, &begin(false)));
        assert!(turn.may_pass("c", proto::FILE_END, br#"{"id":1}"#));
        assert!(turn.may_pass("", proto::FILE_BEGIN, &begin(true)));
        assert!(!turn.may_pass("c", proto::FILE_END, br#"{"id":1}"#));
    }
}
