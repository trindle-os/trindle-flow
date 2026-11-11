//! Carrying copied files from one machine to the other.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::{key, Config};
use crate::copied::Refused;
use crate::proto::{FILE_ABORT, FILE_BEGIN, FILE_END, FILE_PIECE};

pub const CHUNK: usize = 128 * 1024;
pub const MAX_FILES: usize = 32;
pub const MAX_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_PER_HOUR: u64 = 4 * 1024 * 1024 * 1024;
const HOUR: Duration = Duration::from_secs(3600);

pub fn downloads_folder() -> Option<PathBuf> {
    let home = trindle_common::files::home()?;
    let mut base = home.join("Downloads");
    if let Some(Ok(text)) = trindle_common::files::config_home().map(|c| std::fs::read_to_string(c.join("user-dirs.dirs"))) {
        if let Some(v) = text.lines().find_map(|l| l.strip_prefix("XDG_DOWNLOAD_DIR=")) {
            let v = v.trim().trim_matches('"').replace("$HOME", &home.to_string_lossy());
            if v.starts_with('/') {
                base = PathBuf::from(v);
            }
        }
    }
    Some(base.join("Flow"))
}

pub fn temporary_folder() -> PathBuf {
    let runtime = trindle_common::files::private_runtime_dir().ok();
    runtime.unwrap_or_else(crate::config::dir).join("flow-files")
}

pub fn clear_temporary() {
    let _ = std::fs::remove_dir_all(temporary_folder());
}

pub fn destination(cfg: &Config) -> (PathBuf, bool) {
    if !cfg.bool(key::FILES_KEEP) {
        return (temporary_folder(), false);
    }
    let chosen = cfg.str(key::FILES_FOLDER);
    let chosen = chosen.trim();
    if chosen.is_empty() {
        downloads_folder().map_or_else(|| (temporary_folder(), false), |d| (d, true))
    } else {
        (PathBuf::from(chosen), true)
    }
}

pub fn safe_name(name: &str) -> String {
    let base = name.rsplit('/').next().unwrap_or("").trim();
    let mut out = String::new();
    let mut in_bad_run = false;
    for c in base.chars() {
        if c.is_alphanumeric() || "_-. ()[]".contains(c) {
            out.push(c);
            in_bad_run = false;
        } else if !in_bad_run {
            out.push('_');
            in_bad_run = true;
        }
    }
    let mut out = out.trim_start_matches('.').to_string();
    let mut cut = out.len().min(200);
    while !out.is_char_boundary(cut) {
        cut -= 1;
    }
    out.truncate(cut);
    if out.is_empty() { "file".into() } else { out }
}

fn free_name(folder: &Path, name: &str) -> Option<PathBuf> {
    let (mut stem, mut tail) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.to_string(), String::new()),
    };
    if stem.to_lowercase().ends_with(".tar") {
        tail = format!("{}{tail}", &stem[stem.len() - 4..]);
        stem.truncate(stem.len() - 4);
    }
    (1..1000).map(|n| if n == 1 { name.to_string() } else { format!("{stem} ({n}){tail}") })
        .map(|c| folder.join(c))
        .find(|p| !p.exists())
}

pub fn readable(paths: &[String]) -> Vec<String> {
    paths
        .iter()
        .take(MAX_FILES)
        .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= MAX_BYTES))
        .cloned()
        .collect()
}

fn pack_piece(id: u32, data: &[u8]) -> Vec<u8> {
    let mut out = id.to_be_bytes().to_vec();
    out.extend_from_slice(data);
    out
}

pub type Deliver = Arc<dyn Fn(u8, Vec<u8>) -> bool + Send + Sync>;
pub type Log = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Clone)]
pub struct Sender {
    deliver: Deliver,
    log: Log,
    next: Arc<AtomicU32>,
}

impl Sender {
    pub fn new(deliver: Deliver, log: Log) -> Sender {
        Sender { deliver, log, next: Arc::new(AtomicU32::new(0)) }
    }

    pub fn send_copied(&self, paths: &[String]) -> bool {
        let Some(socket) = crate::copied::socket_path() else { return self.send(paths) };
        let me = self.clone();
        let paths = paths.to_vec();
        std::thread::spawn(move || {
            let log = &me.log;
            let mut asked = crate::copied::fetch(&socket, log);
            if matches!(asked, Err(Refused::SlowDown)) {
                std::thread::sleep(crate::copied::RETRY);
                asked = crate::copied::fetch(&socket, log);
            }
            match asked {
                Ok(copies) => {
                    log(format!("file: Trindle handed over {} to send", copies.len()));
                    let files = copies.into_iter().map(|c| (c.name, c.size, c.file)).collect();
                    stream_files(files, &me.deliver, log, &me.next);
                }
                Err(Refused::Nothing) => {}
                Err(why) => {
                    log(format!("file: {}", why.message()));
                    if !paths.is_empty() {
                        me.send(&paths);
                    }
                }
            }
        });
        true
    }

    pub fn send(&self, paths: &[String]) -> bool {
        let ok = readable(paths);
        if ok.len() < paths.len() {
            (self.log)(format!(
                "file: {} left behind: too big, or not an ordinary file (the limit is {} MB)",
                paths.len() - ok.len(),
                MAX_BYTES / (1024 * 1024)
            ));
        }
        if ok.is_empty() {
            return false;
        }
        let (deliver, log, next) = (self.deliver.clone(), self.log.clone(), self.next.clone());
        std::thread::spawn(move || send_all(&ok, &deliver, &log, &next));
        true
    }
}

fn send_all(paths: &[String], deliver: &Deliver, log: &Log, next: &AtomicU32) {
    let mut opened = Vec::new();
    for path in paths {
        let name = path.rsplit('/').next().unwrap_or("file").to_string();
        match File::open(path).and_then(|f| f.metadata().map(|m| (f, m.len()))) {
            Ok((file, size)) => opened.push((name, size, file)),
            Err(e) => return log(format!("file: cannot read {name} ({e})")),
        }
    }
    stream_files(opened, deliver, log, next);
}

fn stream_files(files: Vec<(String, u64, File)>, deliver: &Deliver, log: &Log, next: &AtomicU32) {
    let count = files.len();
    for (n, (name, size, mut file)) in files.into_iter().enumerate() {
        let id = next.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
        let head = json!({"id": id, "name": safe_name(&name), "size": size, "first": n == 0, "last": n + 1 == count});
        if !deliver(FILE_BEGIN, head.to_string().into_bytes()) {
            return;
        }
        let tail = json!({"id": id}).to_string().into_bytes();
        let mut buf = vec![0u8; CHUNK];
        loop {
            let got = match file.read(&mut buf) {
                Ok(0) => break,
                Ok(g) => g,
                Err(e) => {
                    deliver(FILE_ABORT, tail);
                    return log(format!("file: cannot read {name} ({e})"));
                }
            };
            if !deliver(FILE_PIECE, pack_piece(id, &buf[..got])) {
                deliver(FILE_ABORT, tail);
                return log(format!("file: {name} was not sent"));
            }
        }
        if !deliver(FILE_END, tail.clone()) {
            deliver(FILE_ABORT, tail);
            return log(format!("file: {name} was not sent"));
        }
    }
}

struct Incoming {
    file: File,
    path: PathBuf,
    expected: u64,
    written: u64,
    last: bool,
}

pub struct Receiver {
    log: Log,
    open: HashMap<u32, Incoming>,
    batch: Vec<PathBuf>,
    passing: Vec<PathBuf>,
    spoiled: bool,
    hour: (Instant, u64),
}

impl Receiver {
    pub fn new(log: Log) -> Receiver {
        Receiver { log, open: HashMap::new(), batch: Vec::new(), passing: Vec::new(), spoiled: false, hour: (Instant::now(), 0) }
    }

    pub fn begin(&mut self, body: &[u8], where_to: (PathBuf, bool)) {
        let Ok(head) = serde_json::from_slice::<Value>(body) else { return };
        let (Some(id), Some(size)) = (head["id"].as_u64(), head["size"].as_u64()) else { return };
        let Ok(id) = u32::try_from(id) else { return };
        let name = safe_name(head["name"].as_str().unwrap_or("file"));
        let last = head["last"].as_bool().unwrap_or(false);
        if head["first"].as_bool().unwrap_or(self.open.is_empty() && self.batch.is_empty()) {
            let ids: Vec<u32> = self.open.keys().copied().collect();
            for old in ids {
                self.abandon(old);
            }
            self.batch.clear();
            self.spoiled = false;
            for old in self.passing.drain(..) {
                let _ = std::fs::remove_file(old);
            }
        }
        if self.open.contains_key(&id) {
            (self.log)("file: refused, one with the same number is already arriving".into());
            return self.spoil(id);
        }
        if size > MAX_BYTES || self.open.len() >= MAX_FILES {
            (self.log)("file: refused, too big or too many at once".into());
            return self.spoil(id);
        }
        if self.hour.0.elapsed() > HOUR {
            self.hour = (Instant::now(), 0);
        }
        if self.hour.1 + size > MAX_PER_HOUR {
            if self.hour.1 <= MAX_PER_HOUR {
                (self.log)("file: refused, 4 GB has already come in this hour".into());
                self.hour.1 = MAX_PER_HOUR + 1;
            }
            return self.spoil(id);
        }
        self.hour.1 += size;
        let (folder, keep) = where_to;
        let made = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder);
        let opened = made.and_then(|()| {
            let path = free_name(&folder, &name).ok_or_else(|| std::io::Error::other("no free name left"))?;
            let file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path)?;
            Ok((path, file))
        });
        let (path, file) = match opened {
            Ok(x) => x,
            Err(e) => {
                (self.log)(format!("file: cannot write into {} ({e})", folder.display()));
                return self.spoil(id);
            }
        };
        if !keep {
            self.passing.push(path.clone());
        }
        self.open.insert(id, Incoming { file, path, expected: size, written: 0, last });
    }

    pub fn piece(&mut self, body: &[u8]) {
        let Some((id, data)) = body.split_first_chunk::<4>() else { return };
        let id = u32::from_be_bytes(*id);
        let Some(entry) = self.open.get_mut(&id) else { return };
        entry.written += data.len() as u64;
        if entry.written > entry.expected {
            (self.log)("file: more arrived than was promised, dropped".into());
            return self.spoil(id);
        }
        if let Err(e) = entry.file.write_all(data) {
            (self.log)(format!("file: cannot write ({e})"));
            self.spoil(id);
        }
    }

    pub fn abort(&mut self, body: &[u8]) {
        let Some(id) = serde_json::from_slice::<Value>(body).ok().and_then(|h| u32::try_from(h["id"].as_u64()?).ok()) else { return };
        (self.log)("file: the other computer stopped sending, dropped".into());
        self.spoil(id);
    }

    pub fn end(&mut self, body: &[u8]) -> Option<Vec<String>> {
        let head = serde_json::from_slice::<Value>(body).ok()?;
        let id = u32::try_from(head["id"].as_u64()?).ok()?;
        let entry = self.open.remove(&id)?;
        let name = entry.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        drop(entry.file);
        if entry.written != entry.expected {
            (self.log)(format!("file: {name} arrived short, dropped"));
            let _ = std::fs::remove_file(&entry.path);
            self.spoil(id);
            return None;
        }
        (self.log)(format!("file: {name} arrived"));
        self.batch.push(entry.path);
        if !entry.last {
            return None;
        }
        if std::mem::take(&mut self.spoiled) {
            (self.log)("file: not all of the copy arrived, so nothing is pasted".into());
            self.batch.clear();
            return None;
        }
        Some(self.batch.drain(..).map(|p| p.to_string_lossy().into_owned()).collect())
    }

    pub fn handle(&mut self, kind: u8, body: &[u8], cfg: &Config) -> Option<Vec<String>> {
        match kind {
            FILE_BEGIN => self.begin(body, destination(cfg)),
            FILE_PIECE => self.piece(body),
            FILE_ABORT => self.abort(body),
            _ => return self.end(body),
        }
        None
    }

    pub fn forget(&mut self) {
        let ids: Vec<u32> = self.open.keys().copied().collect();
        for id in ids {
            self.abandon(id);
        }
        self.batch.clear();
        self.spoiled = false;
    }

    fn spoil(&mut self, id: u32) {
        self.abandon(id);
        self.batch.clear();
        self.spoiled = true;
    }

    fn abandon(&mut self, id: u32) {
        if let Some(entry) = self.open.remove(&id) {
            let _ = std::fs::remove_file(entry.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    type Sent = Arc<Mutex<Vec<(u8, Vec<u8>)>>>;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name("../../etc/passwd"), "passwd");
        assert_eq!(safe_name("..hidden"), "hidden");
        assert_eq!(safe_name("a;b|c.txt"), "a_b_c.txt");
        assert_eq!(safe_name("naïve café.pdf"), "naïve café.pdf");
        assert_eq!(safe_name(""), "file");
        assert!(safe_name(&"é".repeat(150)).len() <= 200);
    }

    #[test]
    fn files_travel_whole() {
        let base = std::env::temp_dir().join(format!("flow-files-{}", std::process::id()));
        let (src, dst) = (base.join("src"), base.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        let a = src.join("a.tar.gz");
        std::fs::write(&a, vec![7u8; CHUNK * 2 + 5]).unwrap();
        std::fs::write(dst.join("a.tar.gz"), b"already here").unwrap();
        let sent: Sent = Arc::default();
        let s2 = sent.clone();
        let deliver: Deliver = Arc::new(move |k, b| {
            s2.lock().unwrap().push((k, b));
            true
        });
        let log: Log = Arc::new(|_| {});
        send_all(&[a.to_string_lossy().into()], &deliver, &log, &AtomicU32::new(0));
        let mut r = Receiver::new(log);
        let mut landed = None;
        for (k, b) in sent.lock().unwrap().iter() {
            match *k {
                FILE_BEGIN => r.begin(b, (dst.clone(), true)),
                FILE_PIECE => r.piece(b),
                _ => landed = r.end(b),
            }
        }
        let landed = landed.unwrap();
        assert!(landed[0].ends_with("a (2).tar.gz"));
        assert_eq!(std::fs::read(&landed[0]).unwrap(), vec![7u8; CHUNK * 2 + 5]);
        r.begin(br#"{"id":9,"name":"x","size":2,"last":true}"#, (dst.clone(), true));
        r.piece(&pack_piece(9, b"abc"));
        assert!(!dst.join("x").exists());
        let _ = std::fs::remove_dir_all(base);
    }

    fn feed(r: &mut Receiver, sent: &[(u8, Vec<u8>)], dst: &Path) -> Option<Vec<String>> {
        let mut landed = None;
        for (k, b) in sent {
            match *k {
                FILE_BEGIN => r.begin(b, (dst.to_path_buf(), true)),
                FILE_PIECE => r.piece(b),
                FILE_ABORT => r.abort(b),
                _ => landed = r.end(b),
            }
        }
        landed
    }

    #[test]
    fn no_more_than_four_gigabytes_come_in_an_hour() {
        let dst = std::env::temp_dir().join(format!("flow-hour-{}", std::process::id()));
        let said = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = said.clone();
        let mut r = Receiver::new(Arc::new(move |m| log.lock().unwrap().push(m)));
        r.hour.1 = MAX_PER_HOUR - 10;
        let head = |id: u32, size: u64| json!({"id": id, "size": size, "name": "a.txt", "first": false}).to_string().into_bytes();
        r.begin(&head(1, 10), (dst.clone(), true));
        assert!(r.open.contains_key(&1));
        r.begin(&head(2, 1), (dst.clone(), true));
        r.begin(&head(3, 1), (dst.clone(), true));
        assert!(!r.open.contains_key(&2) && !r.open.contains_key(&3));
        assert_eq!(said.lock().unwrap().len(), 1);
        if let Some(earlier) = Instant::now().checked_sub(HOUR + Duration::from_secs(1)) {
            r.hour.0 = earlier;
            r.begin(&head(4, 1), (dst.clone(), true));
            assert!(r.open.contains_key(&4));
        }
        drop(r);
        let _ = std::fs::remove_dir_all(dst);
    }

    #[test]
    fn a_batch_that_breaks_leaves_nothing_behind_and_is_never_pasted() {
        let base = std::env::temp_dir().join(format!("flow-broken-{}", std::process::id()));
        let (src, dst) = (base.join("src"), base.join("dst"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        let (a, b) = (src.join("a.bin"), src.join("b.bin"));
        std::fs::write(&a, vec![1u8; CHUNK * 3]).unwrap();
        std::fs::write(&b, b"bee").unwrap();
        let log: Log = Arc::new(|_| {});

        let sent: Sent = Arc::default();
        let s2 = sent.clone();
        let deliver: Deliver = Arc::new(move |k, body| {
            let mut s = s2.lock().unwrap();
            let pieces = s.iter().filter(|(k, _)| *k == FILE_PIECE).count();
            if k == FILE_PIECE && pieces == 1 {
                return false;
            }
            s.push((k, body));
            true
        });
        send_all(&[a.to_string_lossy().into()], &deliver, &log, &AtomicU32::new(0));
        assert_eq!(sent.lock().unwrap().last().unwrap().0, FILE_ABORT);
        let mut r = Receiver::new(log.clone());
        assert_eq!(feed(&mut r, &sent.lock().unwrap(), &dst), None);
        assert!(r.open.is_empty());
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 0);

        r.begin(br#"{"id":1,"name":"one","size":1,"first":true,"last":false}"#, (dst.clone(), true));
        r.piece(&pack_piece(1, b"1"));
        assert_eq!(r.end(br#"{"id":1}"#), None);
        r.begin(br#"{"id":2,"name":"two","size":1,"first":false,"last":true}"#, (dst.clone(), true));
        r.piece(&pack_piece(2, b"22"));
        assert_eq!(r.end(br#"{"id":2}"#), None);
        let sent: Sent = Arc::default();
        let s2 = sent.clone();
        let deliver: Deliver = Arc::new(move |k, body| {
            s2.lock().unwrap().push((k, body));
            true
        });
        send_all(&[b.to_string_lossy().into()], &deliver, &log, &AtomicU32::new(10));
        let landed = feed(&mut r, &sent.lock().unwrap(), &dst).unwrap();
        assert_eq!(landed.len(), 1);
        assert!(landed[0].ends_with("b.bin"));

        r.begin(br#"{"id":5,"name":"five","size":4,"first":true,"last":false}"#, (dst.clone(), true));
        r.begin(br#"{"id":5,"name":"other","size":4,"first":false,"last":true}"#, (dst.clone(), true));
        assert!(r.open.is_empty() && !dst.join("five").exists() && !dst.join("other").exists());

        for id in 20..20 + MAX_FILES as u32 {
            r.begin(format!(r#"{{"id":{id},"name":"n{id}","size":9,"first":{},"last":false}}"#, id == 20).as_bytes(), (dst.clone(), true));
        }
        assert_eq!(r.open.len(), MAX_FILES);
        r.begin(br#"{"id":99,"name":"fresh","size":1,"first":true,"last":true}"#, (dst.clone(), true));
        assert_eq!(r.open.len(), 1);
        assert!(!dst.join("n20").exists());
        let _ = std::fs::remove_dir_all(base);
    }
}
