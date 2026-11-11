//! Flow, a Trindle plugin: one computer's mouse and keyboard control another.

mod capture;
mod client;
mod clipboard;
mod config;
mod control;
mod copied;
mod discovery;
mod files;
mod layout;
mod link;
mod page;
mod pairing;
mod panel;
mod proto;
mod screen;
mod server;
mod status;
mod tls;
mod unlock;

use std::io::{BufRead, Read};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use config::key;
use files::Log;

static STOP: AtomicBool = AtomicBool::new(false);
static STOP_FD: AtomicI32 = AtomicI32::new(-1);
const WATCH_EVERY: Duration = Duration::from_secs(1);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
    let fd = STOP_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let one = 1u64.to_ne_bytes();
        // SAFETY: write is safe in a signal handler; one is eight readable bytes.
        unsafe { libc::write(fd, one.as_ptr().cast(), 8) };
    }
}

fn stop_bell() -> &'static Bell {
    static BELL: std::sync::OnceLock<Bell> = std::sync::OnceLock::new();
    BELL.get_or_init(|| {
        let bell = Bell::new();
        STOP_FD.store(std::os::fd::AsRawFd::as_raw_fd(&bell.fd), Ordering::Relaxed);
        bell
    })
}

fn catch_stop() {
    stop_bell();
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only stores to atomics and writes to an eventfd,
        // both safe from a signal handler, and it lives for the whole program.
        unsafe {
            libc::signal(sig, on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t);
        }
    }
}

fn say() -> Log {
    Arc::new(|line: String| eprintln!("{line}"))
}

const USAGE: &str = "Flow

  flow run            share or receive the mouse and keyboard, as settings say
  flow settings       print the settings, as JSON
  flow set KEY VALUE  change one setting
  flow forget         unpair and make a new key
  flow allow ID       on the controlling computer: allow the computer asking to pair, with
                      the number it shows, read from standard input
  flow use ID         on the computer to be controlled: ask a computer Flow found to pair
  flow goto ID        on the controlling computer: take the pointer over to that computer
  flow switch         go to the other computer, or come back
  flow home           bring the pointer back to this computer
  flow drop ID        on the controlling computer: disconnect that computer
  flow connect        start sharing again after a disconnect
  flow disconnect     stop sharing until connect, without unpairing
  flow tell           how Flow is getting on, shown in the ring
  flow unlock [ID]    on the controlling computer: unlock that computer's screen with
                      the PIN, read from standard input; says the answer
  flow version        print the version
  flow help           print this list";

fn main() -> ExitCode {
    if config::find_dir().is_none() {
        eprintln!("{}", config::NO_FOLDER);
        return ExitCode::FAILURE;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("unlock") {
        let answer = unlock::ask(args.get(1).map_or("", String::as_str));
        println!("{}", answer.words);
        return if answer.done { ExitCode::SUCCESS } else { ExitCode::FAILURE };
    }
    match command(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn command(args: &[String]) -> Result<(), String> {
    let arg = |i: usize| args.get(i).map(String::as_str);
    match arg(0).unwrap_or("run") {
        "run" => config::claim_running().map(|_held| supervise()),
        "settings" => {
            println!("{}", panel::settings());
            Ok(())
        }
        "set" => match (arg(1), arg(2)) {
            (Some(k), Some(v)) => panel::set(k, v),
            _ => Err("flow set KEY VALUE".into()),
        },
        "forget" => panel::forget(),
        "allow" => match arg(1) {
            Some(id) => pairing::allow(id, &read_number()?).map(|()| page::ring_menu(&status::read())),
            None => Err("flow allow ID".into()),
        },
        "use" => match arg(1) {
            Some(id) => pairing::use_found(id).map(|said| {
                page::tell("Flow", &said);
                println!("{said}");
            }),
            None => Err("flow use ID".into()),
        },
        "goto" | "switch" | "home" => {
            let which = arg(0).unwrap_or("home");
            let note = serde_json::json!({"do": which, "id": arg(1).unwrap_or(""), "at": config::now_secs()});
            config::write("go.json", note.to_string().as_bytes()).map_err(|e| e.to_string())
        }
        "drop" => match arg(1) {
            Some(id) => {
                config::set_value(key::DROP_ID, serde_json::json!(id));
                Ok(())
            }
            None => Err("flow drop ID".into()),
        },
        "connect" => panel::set(key::CONNECTED, "on").map(|()| {
            config::set_value(key::DROPPED, serde_json::json!([]));
            page::ring_menu(&status::read())
        }),
        "disconnect" => panel::set(key::CONNECTED, "off").map(|()| page::ring_menu(&status::read())),
        "tell" => {
            page::tell_status();
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("flow {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("'{other}' is not something Flow knows how to do.\n\n{USAGE}")),
    }
}

pub enum Stop {
    Gone,
    Failed(String),
}

impl From<String> for Stop {
    fn from(e: String) -> Stop {
        Stop::Failed(e)
    }
}

impl From<&str> for Stop {
    fn from(e: &str) -> Stop {
        Stop::Failed(e.into())
    }
}

pub struct Stopping(Arc<Bell>, Option<std::thread::JoinHandle<()>>);

impl Stopping {
    pub fn spawn(work: impl FnOnce(Arc<Bell>) + Send + 'static) -> Stopping {
        let bell = Arc::new(Bell::new());
        let rung = bell.clone();
        Stopping(bell, Some(std::thread::spawn(move || work(rung))))
    }
}

impl Drop for Stopping {
    fn drop(&mut self) {
        self.0.ring();
        if let Some(t) = self.1.take() {
            let _ = t.join();
        }
    }
}

pub struct Bell {
    rung: AtomicBool,
    fd: std::os::fd::OwnedFd,
}

impl Bell {
    fn new() -> Bell {
        // SAFETY: eventfd takes no pointers; a negative answer is checked below.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        assert!(fd >= 0, "Flow cannot make an eventfd");
        // SAFETY: fd was just made and nothing else owns it.
        Bell { rung: AtomicBool::new(false), fd: unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) } }
    }

    pub fn ring(&self) {
        self.rung.store(true, Ordering::Relaxed);
        let one = 1u64.to_ne_bytes();
        // SAFETY: one is eight readable bytes, as an eventfd write needs.
        unsafe { libc::write(std::os::fd::AsRawFd::as_raw_fd(&self.fd), one.as_ptr().cast(), 8) };
    }

    pub fn rung(&self) -> bool {
        self.rung.load(Ordering::Relaxed)
    }

    pub fn wait_for(&self, fd: std::os::fd::BorrowedFd) -> bool {
        self.poll(Some(fd), None)
    }

    pub fn sleep(&self, time: Duration) -> bool {
        self.poll(None, Some(time))
    }

    fn poll(&self, fd: Option<std::os::fd::BorrowedFd>, limit: Option<Duration>) -> bool {
        use std::os::fd::AsRawFd;
        let mut fds = vec![libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        fds.extend(fd.map(|f| libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLIN, revents: 0 }));
        let until = limit.map(|l| Instant::now() + l);
        while !self.rung() {
            let left = until.map_or(-1, |u| u.saturating_duration_since(Instant::now()).as_millis().min(i32::MAX as u128) as i32);
            // SAFETY: fds holds valid pollfd entries and its length is passed with it.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, left) };
            if n >= 0 || std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                break;
            }
        }
        !self.rung()
    }
}

fn read_number() -> Result<String, String> {
    let mut line = String::new();
    std::io::stdin().lock().take(16).read_line(&mut line).map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

fn supervise() {
    catch_stop();
    let log = say();
    let mut last_error = String::new();
    while !STOP.load(Ordering::Relaxed) {
        let running = Arc::new(AtomicBool::new(true));
        page::take_and_tell(&*log);
        if !config::load().bool(key::CONNECTED) {
            status::write(&serde_json::json!({(status::key::RUNNING): true, (status::key::CONNECTED): false, (status::key::OFF): true}));
            while !STOP.load(Ordering::Relaxed) && !config::load().bool(key::CONNECTED) {
                pause(WATCH_EVERY);
            }
            continue;
        }
        page::reflect();
        let cfg = config::load();
        let (controls, discovery) = (cfg.controls(), cfg.bool(key::DISCOVERY));
        let worker = {
            let (running, log) = (running.clone(), log.clone());
            std::thread::spawn(move || if controls { server::run(running, log) } else { client::run(running, log) })
        };
        wait_out(&worker, discovery);
        running.store(false, Ordering::Relaxed);
        match worker.join() {
            Ok(Ok(())) => last_error.clear(),
            Ok(Err(Stop::Gone)) if page::installed() => {
                log("the desktop went away: stopping, to be started afresh".into());
                status::clear();
                std::process::exit(3);
            }
            Ok(Err(e)) => {
                let e = match e {
                    Stop::Gone => "the desktop went away".to_string(),
                    Stop::Failed(e) => e,
                };
                if e != last_error {
                    log(e.clone());
                    last_error = e;
                }
                pause(Duration::from_secs(3));
            }
            Err(_) => {
                log("stopped unexpectedly, starting again".into());
                pause(Duration::from_secs(3));
            }
        }
    }
    status::clear();
}

fn wait_out<T>(worker: &std::thread::JoinHandle<T>, discovery: bool) {
    let mut seen = config::mtime();
    while !worker.is_finished() && !STOP.load(Ordering::Relaxed) {
        pause(WATCH_EVERY);
        if config::mtime() != seen {
            seen = config::mtime();
            if run_is_out_of_date(&config::load(), discovery) {
                break;
            }
        }
    }
}

fn run_is_out_of_date(cfg: &config::Config, discovery: bool) -> bool {
    !cfg.bool(key::CONNECTED) || cfg.bool(key::DISCOVERY) != discovery
}

fn pause(length: Duration) {
    if !STOP.load(Ordering::Relaxed) {
        stop_bell().sleep(length);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_ends_on_disconnect_or_when_looking_is_switched() {
        let mut cfg = config::Config(config::defaults());
        assert!(!run_is_out_of_date(&cfg, true));
        assert!(run_is_out_of_date(&cfg, false));
        cfg.set(key::DISCOVERY, serde_json::json!(false));
        assert!(!run_is_out_of_date(&cfg, false));
        cfg.set(key::CONNECTED, serde_json::json!(false));
        assert!(run_is_out_of_date(&cfg, false));
    }
}
