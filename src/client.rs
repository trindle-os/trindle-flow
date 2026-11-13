//! The computer being controlled: it receives the other computer's mouse and keyboard.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::clipboard::{self, Copied};
use crate::config::{self, key, Config};
use crate::discovery::{self, Seeker};
use crate::files::{self, Log};
use crate::link::{Link, Urgency};
use crate::control::Remote;
use crate::proto::{self, Event, Framer};
use crate::unlock::{self, Answer, Pin};
use crate::{page, pairing, screen, status, tls};

const RECONNECT: Duration = Duration::from_secs(3);
const SILENCE: Duration = Duration::from_secs(15);
const PUBLISH_EVERY: Duration = Duration::from_secs(2);
const LINKED_CHECK: Duration = Duration::from_secs(1);

struct Shared {
    cfg: RwLock<Config>,
    log: Log,
    link: Mutex<Option<Arc<Link>>>,
    remote: Option<Remote>,
    receiver: Mutex<files::Receiver>,
    clipboard: clipboard::Watcher,
    identity_changed: AtomicBool,
}

impl Shared {
    fn cfg(&self) -> std::sync::RwLockReadGuard<'_, Config> {
        self.cfg.read().unwrap_or_else(|e| e.into_inner())
    }

    fn link(&self) -> Option<Arc<Link>> {
        self.link.lock().ok().and_then(|l| l.clone()).filter(|l| l.alive())
    }
}

static UNLOCKING: AtomicBool = AtomicBool::new(false);

pub fn run(running: Arc<AtomicBool>, log: Log) -> Result<(), crate::Stop> {
    page::take_and_tell(&*log);
    let cfg = config::load();
    let reach = screen::size(&cfg).reach();
    let remote = match Remote::start() {
        Ok(r) => Some(r),
        Err(e) => {
            log(e.clone());
            page::tell("Flow cannot control this computer", &e);
            None
        }
    };
    files::clear_temporary();

    let (copied_tx, copied_rx) = mpsc::channel();
    let clipboard = clipboard::Watcher::new(copied_tx, &log);
    clipboard.follow(&cfg);
    let sweep = cfg.str(key::SERVER_HOST).is_empty() && cfg.str(key::ADDRESS).is_empty();
    let seeker = cfg.bool(key::DISCOVERY).then(|| {
        let log = log.clone();
        Seeker::start(sweep, move |m| log(m))
    });
    log(format!(
        "Flow {}; screen {}x{}; looking for the other machine",
        env!("CARGO_PKG_VERSION"),
        reach.0,
        reach.1
    ));

    let me = Arc::new(Shared {
        cfg: RwLock::new(cfg),
        log: log.clone(),
        link: Mutex::new(None),
        remote,
        receiver: Mutex::new(files::Receiver::new(log.clone())),
        clipboard,
        identity_changed: AtomicBool::new(false),
    });
    let seeker = Arc::new(seeker);
    pass_on(&me, copied_rx);
    let _publishing = keep_published(&me, &seeker, &running);
    stay_linked(&me, &seeker, &running)?;
    if let Some(link) = me.link() {
        link.close();
    }
    drop(_publishing);
    status::clear();
    Ok(())
}

fn pass_on(me: &Arc<Shared>, copied: mpsc::Receiver<Copied>) {
    let sender = Arc::new(files::Sender::new(deliver(me), me.log.clone()));
    clipboard::announce_each(
        copied,
        Arc::downgrade(me),
        sender,
        |me| me.link().and_then(|_| clipboard::facts(&me.cfg(), &me.log)),
        |me, m, urgency| {
            if let Some(link) = me.link() {
                link.send(m, urgency);
            }
        },
    );
}

fn keep_published(me: &Arc<Shared>, seeker: &Arc<Option<Seeker>>, running: &Arc<AtomicBool>) -> crate::Stopping {
    let (me, seeker, running) = (me.clone(), seeker.clone(), running.clone());
    crate::Stopping::spawn(move |bell| {
        let mut seen = config::mtime();
        while running.load(Ordering::Relaxed) && !bell.rung() {
            page::take_and_tell(&*me.log);
            if config::mtime() != seen {
                seen = config::mtime();
                let fresh = config::load();
                let moved = fresh.str(key::SERVER_ID) != me.cfg().str(key::SERVER_ID);
                me.clipboard.follow(&fresh);
                if let Ok(mut c) = me.cfg.write() {
                    *c = fresh;
                }
                if moved {
                    if let Some(link) = me.link() {
                        link.close();
                    }
                }
            }
            publish(&me, seeker.as_ref().as_ref());
            bell.sleep(PUBLISH_EVERY);
        }
    })
}

fn stay_linked(me: &Arc<Shared>, seeker: &Arc<Option<Seeker>>, running: &AtomicBool) -> Result<(), crate::Stop> {
    let mut last_said = String::new();
    while running.load(Ordering::Relaxed) {
        if me.cfg().controls() {
            break;
        }
        if me.remote.as_ref().is_some_and(Remote::lost) {
            if let Some(link) = me.link() {
                link.close();
            }
            return Err(crate::Stop::Gone);
        }
        if let Some(link) = me.link().filter(|l| l.quiet_for() > SILENCE) {
            (me.log)("the other computer has gone quiet".into());
            link.close();
        }
        if me.link().is_none() {
            match session(me, seeker.as_ref().as_ref()) {
                Err(e) if e != last_said => {
                    (me.log)(format!("Not connected: {e}{} Trying again.", if e.ends_with('.') { "" } else { "." }));
                    last_said = e;
                }
                Err(_) => {}
                Ok(()) => last_said.clear(),
            }
            publish(me, seeker.as_ref().as_ref());
        }
        std::thread::sleep(if me.link().is_some() { LINKED_CHECK } else { RECONNECT });
    }
    Ok(())
}

fn deliver(me: &Arc<Shared>) -> files::Deliver {
    let me = Arc::downgrade(me);
    Arc::new(move |kind, body| me.upgrade().and_then(|me| me.link()).is_some_and(|l| l.send(proto::pack(kind, &body), Urgency::Wait)))
}

fn publish(me: &Shared, seeker: Option<&Seeker>) {
    let cfg = me.cfg();
    let asking = (!cfg.str(key::ASKING).is_empty()).then(|| {
        json!({"name": cfg.str(key::SERVER_NAME), "number": cfg.str(key::PAIR_CODE)})
    });
    let mut found = seeker.map(|s| s.found(Some("server"))).unwrap_or_default();
    let mut ids = std::collections::HashSet::new();
    found.retain(|f| ids.insert(f["id"].to_string()));
    let found: Vec<Value> = found
        .iter()
        .map(|f| {
json!({"id": f["id"], "name": f.get("name").unwrap_or(&f["host"]), "host": f["host"]})
        })
        .collect();
    status::write(&json!({
        (status::key::ROLE): "controlled",
        (status::key::RUNNING): true,
        (status::key::CONNECTED): me.link().is_some(),
        (status::key::SERVER): cfg.str(key::SERVER_HOST),
        (status::key::SERVER_NAME): cfg.str(key::SERVER_NAME),
        (status::key::IDENTITY_CHANGED): me.identity_changed.load(Ordering::Relaxed),
        (status::key::CLIPBOARD): me.clipboard.get().and_then(|c| c.route.lock().ok().map(|r| r.clone())),
        (status::key::CONTROL): me.remote.is_some(),
        (status::key::FOUND): found,
        (status::key::ASKING): asking,
    }));
}

fn find_hosts(cfg: &Config, servers: &[Value]) -> Vec<(String, u16)> {
    let Ok(own) = u16::try_from(cfg.i64(key::PORT)) else { return Vec::new() };
    let port = |v: &Value| v["port"].as_u64().and_then(|p| u16::try_from(p).ok()).unwrap_or(own);
    let wanted = cfg.str(key::SERVER_ID);
    let mut hosts: Vec<(String, u16)> = servers
        .iter()
        .filter(|e| if wanted.is_empty() { true } else { e["id"] == wanted.as_str() })
        .take(if wanted.is_empty() { 1 } else { usize::MAX })
        .filter_map(|e| Some((e["host"].as_str()?.to_string(), port(e))))
        .collect();
    for host in [cfg.str(key::ADDRESS), cfg.str(key::SERVER_HOST)] {
        if !host.is_empty() && !hosts.iter().any(|(h, _)| *h == host) {
            hosts.push((host, own));
        }
    }
    hosts
}

enum Greet {
    Ask,
    Token,
    Unpaired,
    Changed,
}

fn greeting(cfg: &Config, actual: &str) -> Greet {
    let pinned = cfg.str(key::SERVER_FINGERPRINT).to_lowercase();
    if !cfg.str(key::ASKING).is_empty() {
        Greet::Ask
    } else if pinned.is_empty() {
        Greet::Unpaired
    } else if pinned != actual {
        Greet::Changed
    } else {
        Greet::Token
    }
}

fn session(me: &Arc<Shared>, seeker: Option<&Seeker>) -> Result<(), String> {
    let found = seeker.map(|s| s.found(Some("server"))).unwrap_or_default();
    let hosts = find_hosts(&me.cfg(), &found);
    let mut why = String::from("No other computer found yet.");
    for (i, (host, port)) in hosts.iter().enumerate() {
        let (stream, actual) = match tls::connect(host, *port, Duration::from_secs(10)) {
            Ok(c) => c,
            Err(e) => {
                why = e;
                continue;
            }
        };
        // A stranger answering with the paired id must not hide the real computer at the next address.
        if i + 1 < hosts.len() && matches!(greeting(&me.cfg(), &actual), Greet::Changed) {
            why = format!("{host} is not the computer you paired with.");
            continue;
        }
        return session_with(me, seeker, stream, host, &actual);
    }
    Err(why)
}

fn session_with(me: &Arc<Shared>, seeker: Option<&Seeker>, mut stream: tls::ClientStream, host: &str, actual: &str) -> Result<(), String> {
    let asking = may_greet(me, seeker, host, actual)?;
    let check = say_hello(me, &mut stream, actual, asking)?;
    let framer = hear_welcome(me, seeker, &mut stream, check, host, actual, asking)?;
    start_link(me, stream, framer)?;
    (me.log)("connected".into());
    publish(me, seeker);
    Ok(())
}

fn may_greet(me: &Shared, seeker: Option<&Seeker>, host: &str, actual: &str) -> Result<bool, String> {
    let greet = greeting(&me.cfg(), actual);
    let already_said = me.identity_changed.swap(matches!(greet, Greet::Changed), Ordering::Relaxed);
    match greet {
        Greet::Ask => Ok(true),
        Greet::Token => Ok(false),
        Greet::Unpaired => Err(format!("This computer is not paired with {host}. Pick it in Flow's settings to pair.")),
        Greet::Changed => {
            publish(me, seeker);
            let who = Some(me.cfg().str(key::SERVER_NAME)).filter(|n| !n.is_empty()).unwrap_or(host.to_string());
            let words = page::identity_changed(&who);
            if !already_said {
                page::tell("Flow", &words);
            }
            Err(words)
        }
    }
}

fn say_hello(me: &Shared, stream: &mut tls::ClientStream, actual: &str, asking: bool) -> Result<Option<pairing::Check>, String> {
    let screens = proto::screens_json(&screen::size(&me.cfg()));
    let me_fp = tls::ensure_cert().unwrap_or_default();
    let mut hello = json!({
        "id": discovery::machine_id(), "name": discovery::machine_name(),
        "width": screens["width"], "height": screens["height"], "screens": screens["screens"], "protocol": proto::VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "wants": "movement",
        "fingerprint": me_fp,
    });
    let mut check = None;
    if asking {
        let tie = tls::tie(&stream.conn)?;
        let (c, msg) = pairing::start(&me.cfg().str(key::PAIR_CODE), (&discovery::machine_id(), &me_fp), actual, &tie, false);
        hello["asks_to_pair"] = json!(true);
        hello["pake"] = json!(pairing::b64(&msg));
        check = Some(c);
    } else {
        hello["token"] = json!(me.cfg().str(key::TOKEN));
    }
    stream.write_all(&proto::pack_json(proto::HELLO, &hello)).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    Ok(check)
}

fn hear_welcome(
    me: &Shared,
    seeker: Option<&Seeker>,
    stream: &mut tls::ClientStream,
    mut check: Option<pairing::Check>,
    host: &str,
    actual: &str,
    asking: bool,
) -> Result<Framer, String> {
    let refused = "The other computer is not set to accept this one. There, open Flow's settings and pick this computer.";
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut framer = Framer::for_hello();
    loop {
        let frame = tls::read_small(stream, &mut framer, deadline).map_err(|_| refused.to_string())?;
        let Some((kind, body)) = frame else { return Err(refused.into()) };
        match (kind, check.take()) {
            (proto::PAIR, Some(c)) => {
                let said: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let agreed = c.finish(&pairing::unb64(&said["pake"])).filter(|a| a.fits(true, &pairing::unb64(&said["proof"])));
                let Some(agreed) = agreed else {
                    let (fresh, words) = pairing::missed();
                    if let Ok(mut c) = me.cfg.write() {
                        *c = fresh;
                    }
                    page::tell("Flow", &words);
                    publish(me, seeker);
                    return Err(words);
                };
                let mine = json!({"proof": pairing::b64(&agreed.proof(false))});
                stream.write_all(&proto::pack_json(proto::PAIR, &mine)).map_err(|e| e.to_string())?;
                stream.flush().map_err(|e| e.to_string())?;
            }
            (proto::HELLO, None) => {
                accept_hello(me, &body, host, actual);
                if asking {
                    let fresh = config::set_value(key::ASKING, json!(""));
                    let who = Some(fresh.str(key::SERVER_NAME)).filter(|n| !n.is_empty()).unwrap_or(host.to_string());
                    page::tell("Flow", &format!("Paired with {who}. It connects by itself from now on."));
                    if let Ok(mut c) = me.cfg.write() {
                        *c = fresh;
                    }
                }
                break;
            }
            (proto::HELLO, Some(_)) => {
                return Err(format!("{host} runs an older Flow that cannot pair with this one. Update Flow there."));
            }
            (_, c) => check = c,
        }
    }
    Ok(framer)
}

fn start_link(me: &Arc<Shared>, stream: tls::ClientStream, framer: Framer) -> Result<(), String> {
    let (framer, rest) = framer.trusted();

    let tls::ClientStream { conn, sock } = stream;
    let _ = sock.set_read_timeout(None);
    let _ = sock.set_write_timeout(None);
    let (on_me, off_me) = (me.clone(), me.clone());
    let link = Link::start(
        conn.into(),
        sock,
        framer,
        rest,
        move |link, kind, body| dispatch(&on_me, link, kind, body),
        move |why| {
            if let Ok(mut r) = off_me.receiver.lock() {
                r.forget();
            }
            if let Some(remote) = &off_me.remote {
                remote.release_all();
            }
            (off_me.log)(format!("disconnected ({why})"));
        },
    )
    .map_err(|e| e.to_string())?;
    link.touch();
    if let Ok(mut l) = me.link.lock() {
        *l = Some(link);
    }
    Ok(())
}

fn accept_hello(me: &Shared, body: &[u8], host: &str, fingerprint: &str) {
    let info: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let text = |k: &str| info[k].as_str().unwrap_or_default().to_string();
    let mut changed = Vec::new();
    {
        let cfg = me.cfg();
        for (name, value) in [
            (key::TOKEN, text("token")),
            (key::SERVER_HOST, host.to_string()),
            (key::SERVER_FINGERPRINT, fingerprint.to_string()),
            (key::SERVER_ID, text("id")),
            (key::SERVER_NAME, page::plain(&text("name"), 40)),
        ] {
            if !value.is_empty() && cfg.str(name) != value {
                changed.push((name, value));
            }
        }
    }
    let paired = changed.iter().any(|(k, _)| *k == key::TOKEN);
    for (name, value) in changed {
        let fresh = config::set_value(name, json!(value));
        if let Ok(mut c) = me.cfg.write() {
            *c = fresh;
        }
    }
    if paired {
        let who = Some(page::plain(&text("name"), 40)).filter(|n| !n.is_empty()).unwrap_or_else(|| host.to_string());
        (me.log)(format!("paired with {who}"));
    }
}

fn dispatch(me: &Shared, link: &Arc<Link>, kind: u8, body: Vec<u8>) {
    let (clip, share) = {
        let cfg = me.cfg();
        (cfg.bool(key::SHARE_CLIPBOARD), cfg.bool(key::SHARE_FILES))
    };
    match kind {
        proto::INPUT => replay(me, &proto::unpack_events(&body)),
        proto::ENTER => {
            let at: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            if let (Some(x), Some(y), Some(remote)) = (at["x"].as_i64(), at["y"].as_i64(), &me.remote) {
                let screens = screen::recent().unwrap_or_else(|| screen::fallback(&me.cfg()));
                remote.place(x as i32, y as i32, &screens);
            }
        }
        proto::LEAVE => {
            if let Some(remote) = &me.remote {
                remote.release_all();
            }
        }
        proto::PING => {
            let screens = screen::recent().unwrap_or_else(|| screen::fallback(&me.cfg()));
            link.send(proto::pong(&screens), Urgency::Needed);
        }
        proto::UNLOCK => unlock_asked(me, link, body),
        proto::CLIPBOARD if clip => {
            if let Ok(text) = String::from_utf8(body) {
                clipboard::landed(me.clipboard.get().as_deref(), &me.log, Copied::Text(text));
            }
        }
        proto::PICTURE if clip => {
            clipboard::landed(me.clipboard.get().as_deref(), &me.log, Copied::Picture(body));
        }
        proto::FILE_BEGIN | proto::FILE_PIECE | proto::FILE_END | proto::FILE_ABORT if share => {
            let batch = me.receiver.lock().ok().and_then(|mut r| r.handle(kind, &body, &me.cfg()));
            if let Some(batch) = batch {
                clipboard::landed(me.clipboard.get().as_deref(), &me.log, Copied::Files(batch));
            }
        }
        _ => {}
    }
}

fn unlock_asked(me: &Shared, link: &Arc<Link>, mut body: Vec<u8>) {
    if UNLOCKING.swap(true, Ordering::AcqRel) {
        unlock::wipe_bytes(&mut body);
        link.send(proto::pack_json(proto::UNLOCKED, &Answer::no("Another unlock attempt is still running.").to_json()), Urgency::Needed);
        return;
    }
    let (link, log) = (link.clone(), me.log.clone());
    std::thread::spawn(move || {
        let answer = match String::from_utf8(std::mem::take(&mut body)) {
            Ok(text) => match Pin::new(text) {
                Ok(pin) => unlock::with_pin(&pin),
                Err(words) => Answer::no(words),
            },
            Err(e) => {
                unlock::wipe_bytes(&mut e.into_bytes());
                Answer::no(unlock::NOT_FOUR_DIGITS)
            }
        };
        log(format!("unlock: the other computer asked; {}", if answer.done { "unlocked".to_string() } else { format!("not unlocked: {}", answer.words) }));
        UNLOCKING.store(false, Ordering::Release);
        link.send(proto::pack_json(proto::UNLOCKED, &answer.to_json()), Urgency::Needed);
    });
}

fn replay(me: &Shared, events: &[Event]) {
    if events.is_empty() {
        return;
    }
    let Some(remote) = &me.remote else { return };
    remote.emit(events);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(cfg: &Config, found: &[Value]) -> Vec<String> {
        find_hosts(cfg, found).into_iter().map(|(h, _)| h).collect()
    }

    #[test]
    fn the_typed_address_comes_before_the_last_one_used() {
        let mut cfg = Config(config::defaults());
        assert!(hosts(&cfg, &[]).is_empty());
        cfg.set(key::SERVER_HOST, json!("192.168.1.20"));
        assert_eq!(hosts(&cfg, &[]), ["192.168.1.20"]);
        cfg.set(key::ADDRESS, json!("100.64.0.7"));
        assert_eq!(hosts(&cfg, &[]), ["100.64.0.7", "192.168.1.20"]);
    }

    #[test]
    fn a_stranger_using_the_paired_id_does_not_hide_the_real_computer() {
        let mut cfg = Config(config::defaults());
        cfg.set(key::SERVER_ID, json!("desk1"));
        cfg.set(key::SERVER_HOST, json!("192.168.1.20"));
        let found = [
            json!({"id": "desk1", "host": "192.168.1.66"}),
            json!({"id": "desk1", "host": "192.168.1.20"}),
            json!({"id": "other", "host": "192.168.1.30"}),
        ];
        assert_eq!(hosts(&cfg, &found), ["192.168.1.66", "192.168.1.20"]);
        cfg.set(key::SERVER_ID, json!(""));
        assert_eq!(hosts(&cfg, &found[2..]), ["192.168.1.30", "192.168.1.20"]);
    }

    #[test]
    fn the_token_goes_only_to_the_computer_pinned() {
        let mut cfg = Config(serde_json::Map::new());
        assert!(matches!(greeting(&cfg, "aa"), Greet::Unpaired));
        cfg.set(key::SERVER_FINGERPRINT, json!("AA"));
        assert!(matches!(greeting(&cfg, "aa"), Greet::Token));
        assert!(matches!(greeting(&cfg, "bb"), Greet::Changed));
        cfg.set(key::ASKING, json!("desk1"));
        assert!(matches!(greeting(&cfg, "bb"), Greet::Ask));
    }

    #[test]
    fn a_changed_identity_is_refused_and_said_until_the_real_one_returns() {
        let _home = crate::config::tests::TestHome::new();
        let mut cfg = config::load();
        cfg.set(key::SERVER_FINGERPRINT, json!("aa"));
        cfg.set(key::SERVER_NAME, json!("Desk"));
        let log: Log = Arc::new(|_| {});
        let me = Shared {
            cfg: RwLock::new(cfg),
            log: log.clone(),
            link: Mutex::new(None),
            remote: None,
            receiver: Mutex::new(files::Receiver::new(log.clone())),
            clipboard: clipboard::Watcher::new(mpsc::channel().0, &log),
            identity_changed: AtomicBool::new(false),
        };
        assert!(may_greet(&me, None, "10.0.0.2", "bb").unwrap_err().contains("Desk is not the computer it was"));
        assert_eq!(me.cfg().str(key::SERVER_FINGERPRINT), "aa");
        assert_eq!(crate::status::read()[status::key::IDENTITY_CHANGED], true);
        publish(&me, None);
        assert_eq!(crate::status::read()[status::key::IDENTITY_CHANGED], true);
        assert!(!may_greet(&me, None, "10.0.0.2", "aa").unwrap());
        publish(&me, None);
        assert_eq!(crate::status::read()[status::key::IDENTITY_CHANGED], false);
    }
}
