//! Who may connect: greeting a computer, checking its token or number, and joining it to the others.

use std::io::Write;
use std::net::{IpAddr, SocketAddr, TcpStream};

use rustls::{ServerConfig, ServerConnection, StreamOwned};

use super::*;
use crate::pairing;
use crate::proto::Framer;

const ACCEPT_RETRY: Duration = Duration::from_millis(200);
const GREETING_TIME: Duration = Duration::from_secs(10);
const GREETING_PER_ADDRESS: usize = 2;
const GREETING_AT_ONCE: usize = 8;

#[derive(Default)]
struct Greeting(Mutex<Vec<IpAddr>>);

struct GreetingPlace(Arc<Greeting>, IpAddr);

impl Greeting {
    fn enter(self: &Arc<Self>, ip: IpAddr) -> Option<GreetingPlace> {
        let mut now = lock(&self.0);
        if now.len() >= GREETING_AT_ONCE || now.iter().filter(|a| **a == ip).count() >= GREETING_PER_ADDRESS {
            return None;
        }
        now.push(ip);
        Some(GreetingPlace(self.clone(), ip))
    }
}

impl Drop for GreetingPlace {
    fn drop(&mut self) {
        let mut now = lock(&self.0 .0);
        if let Some(i) = now.iter().position(|a| *a == self.1) {
            now.swap_remove(i);
        }
    }
}

pub(super) fn accept_loop(me: Arc<Shared>, listener: TcpListener, tls_cfg: Arc<ServerConfig>, running: Arc<AtomicBool>, bell: Arc<crate::Bell>) {
    if listener.set_nonblocking(true).is_err() {
        (me.log)("cannot listen for the other computer".into());
        return;
    }
    let greeting = Arc::new(Greeting::default());
    while running.load(Ordering::Relaxed) && bell.wait_for(listener.as_fd()) {
        match listener.accept() {
            Ok((sock, addr)) => {
                if !discovery::may_connect(addr.ip()) {
                    continue;
                }
                let Some(place) = greeting.enter(addr.ip()) else { continue };
                let (me, tls_cfg) = (me.clone(), tls_cfg.clone());
                std::thread::spawn(move || {
                    if let Err(e) = greet(&me, tls_cfg, sock, addr, place) {
                        (me.log)(format!("refused a connection from {}: {e}", addr.ip()));
                    }
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => {
                bell.sleep(ACCEPT_RETRY);
            }
        }
    }
}

type Hello = (Value, Framer, Way);

enum Way {
    Token,
    Number(String),
}

fn read_hello(me: &Shared, stream: &mut tls::ServerStream, deadline: Instant) -> Result<Option<Hello>, String> {
    let mut framer = Framer::for_hello();
    let Some((kind, body)) = tls::read_small(stream, &mut framer, deadline)? else { return Ok(None) };
    if kind != proto::HELLO {
        return Ok(None);
    }
    let hello: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Ok(admit(me, &hello)?.map(|way| (hello, framer, way)))
}

fn admit(me: &Shared, hello: &Value) -> Result<Option<Way>, String> {
    let id = clean_id(hello["id"].as_str().unwrap_or(""));
    if id.is_empty() {
        return Ok(None);
    }
    let who = clean_name(hello["name"].as_str().unwrap_or(""), "another computer");
    let older = |theirs: &Value| {
        let theirs: String = theirs.as_str().unwrap_or("an older version").chars().filter(|c| c.is_ascii_alphanumeric() || ".-+~".contains(*c)).take(24).collect();
        format!("{who} is running Flow {theirs} and this computer Flow {}. They cannot work together: update the older one.", env!("CARGO_PKG_VERSION"))
    };
    if hello["protocol"].as_i64().unwrap_or(1) != proto::VERSION {
        (me.log)(older(&hello["version"]));
        return Ok(None);
    }
    let dropped = me.cfg().0.get(key::DROPPED).and_then(Value::as_array).is_some_and(|d| d.iter().any(|v| v.as_str().map(clean_id).as_deref() == Some(id.as_str())));
    if dropped {
        (me.log)(format!("refused {who}: disconnected from Flow's menu until Connect"));
        return Ok(None);
    }
    if holds_token(&me.cfg(), &id, hello["token"].as_str().unwrap_or("")) {
        return Ok(Some(Way::Token));
    }
    if hello["asks_to_pair"].as_bool() == Some(true) {
        if hello["pake"].as_str().is_none() {
            (me.log)(older(&hello["version"]));
            return Ok(None);
        }
        if let Some(code) = pairing::take_allowance(&id) {
            return Ok(Some(Way::Number(code)));
        }
        me.asked(&id, &who);
        return Ok(None);
    }
    (me.log)(format!("refused {who}: not paired with this computer"));
    Ok(None)
}

fn holds_token(cfg: &Config, id: &str, theirs: &str) -> bool {
    if let Some(own) = cfg.0.get(key::TOKENS).and_then(|t| t.get(id)).and_then(Value::as_str) {
        return same_secret(theirs.as_bytes(), own.as_bytes());
    }
    let shared = cfg.str(key::TOKEN);
    !shared.is_empty()
        && cfg.0.get(key::COMPUTERS).and_then(|c| c.get(id)).is_some()
        && same_secret(theirs.as_bytes(), shared.as_bytes())
}

fn token_for(me: &Shared, id: &str) -> String {
    let _held = config::edit_lock();
    let mut fresh = config::load();
    let mut tokens = fresh.0.get(key::TOKENS).and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(own) = tokens.get(id).and_then(Value::as_str) {
        return own.to_string();
    }
    let own = config::random_token();
    tokens.insert(id.to_string(), json!(own));
    fresh.set(key::TOKENS, Value::Object(tokens));
    if let Err(e) = config::save(&fresh) {
        (me.log)(format!("could not save the settings: {e}"));
    }
    if let Ok(mut c) = me.cfg.write() {
        *c = fresh;
    }
    own
}

fn number_matches(stream: &mut tls::ServerStream, framer: &mut Framer, hello: &Value, code: &str, deadline: Instant) -> Result<bool, String> {
    let asking = (hello["id"].as_str().unwrap_or(""), hello["fingerprint"].as_str().unwrap_or(""));
    let tie = tls::tie(&stream.conn)?;
    let (check, mine) = pairing::start(code, asking, &tls::ensure_cert()?, &tie, true);
    let Some(agreed) = check.finish(&pairing::unb64(&hello["pake"])) else { return Ok(false) };
    let said = json!({"pake": pairing::b64(&mine), "proof": pairing::b64(&agreed.proof(true))});
    tls::in_time(&stream.sock, deadline)?;
    stream.write_all(&proto::pack_json(proto::PAIR, &said)).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    let Some((kind, body)) = tls::read_small(stream, framer, deadline)? else { return Ok(false) };
    let theirs: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Ok(kind == proto::PAIR && agreed.fits(false, &pairing::unb64(&theirs["proof"])))
}

fn greet(me: &Arc<Shared>, tls_cfg: Arc<ServerConfig>, sock: TcpStream, addr: SocketAddr, place: GreetingPlace) -> Result<(), String> {
    tls::tune(&sock);
    let deadline = Instant::now() + GREETING_TIME;
    let mut stream = handshake(tls_cfg, sock, deadline)?;
    let Some((hello, mut framer, way)) = read_hello(me, &mut stream, deadline)? else { return Ok(()) };
    if let Way::Number(code) = way {
        if !paired(me, &mut stream, &mut framer, &hello, &code, deadline)? {
            return Ok(());
        }
    }
    let (framer, rest) = framer.trusted();
    drop(place);

    let ip = addr.ip().to_string();
    let raw_id = match &hello["id"] {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    let id = clean_id(&raw_id);
    if id.is_empty() {
        return Err("it gave no usable name for itself".into());
    }
    let name = clean_name(hello["name"].as_str().unwrap_or(""), &ip);
    remember(me, &id, &name, &ip, hello["fingerprint"].as_str().unwrap_or(""));

    let mut reply = proto::screens_json(&crate::screen::size(&me.cfg()));
    reply["token"] = json!(token_for(me, &id));
    reply["name"] = json!(discovery::machine_name());
    reply["id"] = json!(discovery::machine_id());
    stream.write_all(&proto::pack_json(proto::HELLO, &reply)).map_err(|e| e.to_string())?;
    stream.flush().map_err(|e| e.to_string())?;
    join(me, stream, framer, rest, &hello, &id, &name)?;
    (me.log)(format!("connected: {name} at {ip}"));
    Ok(())
}

fn handshake(tls_cfg: Arc<ServerConfig>, sock: TcpStream, deadline: Instant) -> Result<tls::ServerStream, String> {
    let conn = ServerConnection::new(tls_cfg).map_err(|e| e.to_string())?;
    let mut stream = StreamOwned::new(conn, sock);
    while stream.conn.is_handshaking() {
        tls::in_time(&stream.sock, deadline)?;
        stream.conn.complete_io(&mut stream.sock).map_err(|e| e.to_string())?;
    }
    Ok(stream)
}

fn paired(me: &Shared, stream: &mut tls::ServerStream, framer: &mut Framer, hello: &Value, code: &str, deadline: Instant) -> Result<bool, String> {
    let who = clean_name(hello["name"].as_str().unwrap_or(""), "another computer");
    if !number_matches(stream, framer, hello, code, deadline)? {
        (me.log)(format!("refused {who}: the number typed here did not match the one it shows"));
        page::tell("Flow", &format!("The number typed did not match the one {who} shows. Type it again."));
        return Ok(false);
    }
    lock(&me.asking).retain(|(_, v)| v["id"] != clean_id(hello["id"].as_str().unwrap_or("")).as_str());
    (me.log)(format!("paired with {who}"));
    Ok(true)
}

fn join(me: &Arc<Shared>, stream: tls::ServerStream, framer: Framer, rest: Vec<(u8, Vec<u8>)>, hello: &Value, id: &str, name: &str) -> Result<(), String> {
    let StreamOwned { conn, sock } = stream;
    let _ = sock.set_read_timeout(None);
    let _ = sock.set_write_timeout(None);
    let (on_me, off_me, on_id, off_id) = (me.clone(), me.clone(), id.to_string(), id.to_string());
    let link = Link::start(
        conn.into(),
        sock,
        framer,
        rest,
        move |link, kind, body| arrived(&on_me, &on_id, link, kind, body),
        move |why| {
            let _ = off_me.wake.send(Wake::Closed(off_id, why));
        },
    )
    .map_err(|e| e.to_string())?;
    link.touch();
    let layout = Layout::from_json(&hello["screens"], hello["width"].as_i64(), hello["height"].as_i64());
    let peer = Peer { id: id.to_string(), name: name.to_string(), layout, link };
    let mut peers = lock(&me.peers);
    for old in peers.iter().filter(|p| p.id == id) {
        old.link.close();
    }
    peers.retain(|p| p.id != id);
    peers.push(peer);
    Ok(())
}

fn remember(me: &Shared, id: &str, name: &str, ip: &str, fingerprint: &str) {
    let _held = config::edit_lock();
    let mut fresh = config::load();
    let mut changed = false;
    let mut put = |cfg: &mut Config, key: &str, value: &str| {
        if !value.is_empty() && cfg.str(key) != value {
            cfg.set(key, json!(value));
            changed = true;
        }
    };
    if !fingerprint.is_empty() {
        for (name, value) in [(key::SERVER_ID, id), (key::SERVER_NAME, name), (key::SERVER_HOST, ip), (key::SERVER_FINGERPRINT, fingerprint)] {
            put(&mut fresh, name, value);
        }
    }
    let mut computers = fresh.0.get(key::COMPUTERS).and_then(Value::as_object).cloned().unwrap_or_default();
    if computers.get(id).and_then(Value::as_str) != Some(name) {
        computers.insert(id.to_string(), json!(name));
        fresh.set(key::COMPUTERS, Value::Object(computers));
        changed = true;
    }
    if changed {
        config::save_or_tell(&fresh);
        if let Ok(mut c) = me.cfg.write() {
            *c = fresh;
        }
    }
}

pub(super) fn arrived(me: &Shared, id: &str, link: &Arc<Link>, kind: u8, body: Vec<u8>) {
    let (clip, share) = {
        let cfg = me.cfg();
        (cfg.bool(key::SHARE_CLIPBOARD), cfg.bool(key::SHARE_FILES))
    };
    match kind {
        proto::PING => {
            link.send(proto::pack(proto::PONG, b""), Urgency::Needed);
        }
        proto::PONG => {
            if let Some(layout) = proto::pong_layout(&body) {
                if let Some(p) = lock(&me.peers).iter_mut().find(|p| p.id == id && Arc::ptr_eq(&p.link, link)) {
                    p.layout = layout;
                }
            }
        }
        proto::UNLOCKED => {
            let said: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            if let Some((who, tx)) = lock(&me.unlocking).as_ref() {
                if who == id {
                    let _ = tx.send(Answer::from_json(&said));
                }
            }
        }
        proto::CLIPBOARD if clip => {
            let text = String::from_utf8_lossy(&body).into_owned();
            me.to_everyone(&proto::pack(proto::CLIPBOARD, &body), Some(id), Urgency::Needed);
            clipboard::landed(me.clipboard.get().as_deref(), &me.log, Copied::Text(text));
        }
        proto::PICTURE if clip => {
            me.to_everyone(&proto::pack(proto::PICTURE, &body), Some(id), Urgency::Spare);
            clipboard::landed(me.clipboard.get().as_deref(), &me.log, Copied::Picture(body));
        }
        proto::FILE_BEGIN | proto::FILE_PIECE | proto::FILE_END | proto::FILE_ABORT if share => {
            let _ = me.relay.send((id.to_string(), kind, body.clone()));
            let batch = {
                let mut all = lock(&me.receivers);
                let r = all.entry(id.to_string()).or_insert_with(|| files::Receiver::new(me.log.clone()));
                r.handle(kind, &body, &me.cfg())
            };
            if let Some(batch) = batch {
                clipboard::landed(me.clipboard.get().as_deref(), &me.log, Copied::Files(batch));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::tests::shared;
    use std::io::Read;

    #[test]
    fn only_a_few_may_greet_at_once_and_each_place_is_given_back() {
        let g = Arc::new(Greeting::default());
        let (a, b): (IpAddr, IpAddr) = ("192.0.2.1".parse().unwrap(), "192.0.2.2".parse().unwrap());
        let first = g.enter(a).unwrap();
        let _second = g.enter(a).unwrap();
        assert!(g.enter(a).is_none(), "one address may not take every place");
        drop(first);
        let _again = g.enter(a).unwrap();
        let others: Vec<_> = (0..GREETING_AT_ONCE - 2).map(|_| g.enter(b).or_else(|| g.enter(IpAddr::from([192, 0, 2, 3])))).collect();
        assert!(others.iter().filter(|p| p.is_some()).count() <= GREETING_AT_ONCE - 2);
        let many: Vec<_> = (10..40u8).filter_map(|i| g.enter(IpAddr::from([192, 0, 2, i]))).collect();
        assert_eq!(lock(&g.0).len(), GREETING_AT_ONCE);
        drop(many);
        drop(others);
        assert_eq!(lock(&g.0).len(), 2);
    }

    fn pair_over_tls(typed: &str, shown: &str) -> (bool, bool) {
        let server = tls::server_config().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let typed = typed.to_string();
        let t = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let mut s = StreamOwned::new(ServerConnection::new(server).unwrap(), sock);
            let deadline = Instant::now() + Duration::from_secs(5);
            while s.conn.is_handshaking() {
                s.conn.complete_io(&mut s.sock).unwrap();
            }
            let mut framer = Framer::for_hello();
            let (_, body) = tls::read_small(&mut s, &mut framer, deadline).unwrap().unwrap();
            let hello: Value = serde_json::from_slice(&body).unwrap();
            number_matches(&mut s, &mut framer, &hello, &typed, deadline).unwrap_or(false)
        });
        let (mut c, fp) = tls::connect("127.0.0.1", port, Duration::from_secs(5)).unwrap();
        let (check, msg) = pairing::start(shown, ("laptop1", "aa"), &fp, &tls::tie(&c.conn).unwrap(), false);
        let hello = json!({"id": "laptop1", "fingerprint": "aa", "pake": pairing::b64(&msg)});
        c.write_all(&proto::pack_json(proto::HELLO, &hello)).unwrap();
        c.flush().unwrap();
        let mut framer = Framer::for_hello();
        let (kind, body) = tls::read_small(&mut c, &mut framer, Instant::now() + Duration::from_secs(5)).unwrap().unwrap();
        assert_eq!(kind, proto::PAIR);
        let said: Value = serde_json::from_slice(&body).unwrap();
        let agreed = check.finish(&pairing::unb64(&said["pake"])).filter(|a| a.fits(true, &pairing::unb64(&said["proof"])));
        if let Some(a) = &agreed {
            c.write_all(&proto::pack_json(proto::PAIR, &json!({"proof": pairing::b64(&a.proof(false))}))).unwrap();
            c.flush().unwrap();
        }
        drop(c);
        (t.join().unwrap(), agreed.is_some())
    }

    #[test]
    fn pairing_over_a_real_connection_needs_the_number_shown() {
        let _home = crate::config::tests::TestHome::new();
        assert_eq!(pair_over_tls("4821", "4821"), (true, true));
        assert_eq!(pair_over_tls("4821", "4812"), (false, false));
    }

    fn way(me: &Shared, hello: Value) -> String {
        match admit(me, &hello).unwrap() {
            Some(Way::Token) => "token".into(),
            Some(Way::Number(code)) => format!("number {code}"),
            None => "refused".into(),
        }
    }

    #[test]
    fn only_a_computer_with_the_token_or_a_number_typed_here_may_in() {
        let _home = crate::config::tests::TestHome::new();
        let (me, said) = shared(json!({
            "token": "old-shared",
            "tokens": {"lap1": "lap1-key", "desk2": "desk2-key"},
            "computers": {"lap1": "Desk", "desk2": "Desk 2", "old1": "Old"},
            "dropped": ["desk2"],
        }));
        let hello = |id: &str, extra: Value| {
            let mut h = json!({"id": id, "name": "Desk", "protocol": proto::VERSION, "version": env!("CARGO_PKG_VERSION")});
            h.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            h
        };
        let last = || lock(&said).last().cloned().unwrap_or_default();

        assert_eq!(way(&me, hello("lap1", json!({"token": "lap1-key"}))), "token");
        assert_eq!(way(&me, hello("", json!({"token": "lap1-key"}))), "refused");
        assert_eq!(way(&me, hello("lap1", json!({"token": "wrong-key"}))), "refused");
        assert!(last().contains("not paired"), "{}", last());

        assert_eq!(way(&me, hello("lap1", json!({"token": "old-shared"}))), "refused");
        assert_eq!(way(&me, hello("old1", json!({"token": "old-shared"}))), "token");
        assert_eq!(way(&me, hello("new1", json!({"token": "old-shared"}))), "refused");

        for id in ["desk2", "desk2/", "de sk2", "desk3"] {
            assert_eq!(way(&me, hello(id, json!({"token": "desk2-key"}))), "refused", "{id}");
        }
        assert!(lock(&said).iter().filter(|l| l.contains("disconnected")).count() == 3);

        assert_eq!(way(&me, json!({"id": "lap1", "name": "Desk", "protocol": 1, "version": "2.4.0", "token": "lap1-key"})), "refused");
        assert!(last().contains("Flow 2.4.0") && last().contains("update"), "{}", last());

        assert_eq!(way(&me, hello("lap2", json!({"asks_to_pair": true, "version": "2.5.0"}))), "refused");
        assert!(last().contains("Flow 2.5.0"), "{}", last());

        let asking = hello("lap2", json!({"asks_to_pair": true, "pake": "AAAA"}));
        assert_eq!(way(&me, asking.clone()), "refused");
        assert_eq!(me.asking_now()[0]["id"], "lap2");

        pairing::allow("lap3", "4821").unwrap();
        assert_eq!(way(&me, asking.clone()), "refused");
        pairing::allow("lap2", "4821").unwrap();
        config::set_value(key::ALLOWED_UNTIL, json!(1));
        assert_eq!(way(&me, asking.clone()), "refused");

        pairing::allow("lap2", "4821").unwrap();
        assert_eq!(way(&me, asking.clone()), "number 4821");
        assert_eq!(way(&me, asking), "refused");
    }

    #[test]
    fn each_computer_is_given_a_key_of_its_own_until_unpaired() {
        let _home = crate::config::tests::TestHome::new();
        let (me, _) = shared(json!({}));
        let first = token_for(&me, "lap1");
        assert_eq!(first.len(), 32);
        assert_eq!(token_for(&me, "lap1"), first);
        assert_ne!(token_for(&me, "lap2"), first);
        assert!(holds_token(&me.cfg(), "lap1", &first) && !holds_token(&me.cfg(), "lap2", &first));
        let mut cfg = config::load();
        assert_eq!(cfg.0["tokens"]["lap1"], first.as_str());
        pairing::unpair_into(&mut cfg);
        assert!(!holds_token(&cfg, "lap1", &first));
    }

    #[test]
    fn a_slow_greeting_runs_out_of_time() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let _c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
        let (mut sock, _) = l.accept().unwrap();
        let deadline = Instant::now() + Duration::from_millis(300);
        let started = Instant::now();
        tls::in_time(&sock, deadline).unwrap();
        assert!(sock.read(&mut [0u8; 1]).is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(350));
        assert!(tls::in_time(&sock, deadline).is_err());
    }
}
