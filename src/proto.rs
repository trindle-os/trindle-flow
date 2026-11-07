//! The small framed protocol spoken over one TLS connection, version 2.

use crate::layout::Layout;

pub const VERSION: i64 = 2;

pub const HELLO: u8 = 1;
pub const INPUT: u8 = 2;
pub const CLIPBOARD: u8 = 3;
pub const ENTER: u8 = 4;
pub const LEAVE: u8 = 5;
pub const PING: u8 = 6;
pub const PONG: u8 = 7;
pub const PICTURE: u8 = 8;
pub const FILE_BEGIN: u8 = 9;
pub const FILE_PIECE: u8 = 10;
pub const FILE_END: u8 = 11;
pub const UNLOCK: u8 = 12;
pub const UNLOCKED: u8 = 13;
pub const PAIR: u8 = 14;
pub const FILE_ABORT: u8 = 15;

pub const MAX_FRAME: usize = 40 * 1024 * 1024;
pub const MAX_HELLO: usize = 16 * 1024;
const HDR: usize = 5;
const EVT: usize = 7;

pub type Event = (u8, u16, i32);

pub const EV_KEY: u8 = 0x01;
pub const EV_REL: u8 = 0x02;
pub const REL_X: u16 = 0;
pub const REL_Y: u16 = 1;
pub const REL_HWHEEL: u16 = 6;
pub const REL_WHEEL: u16 = 8;

pub fn pack(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HDR + body.len());
    out.push(kind);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    out
}

pub fn pack_json(kind: u8, value: &serde_json::Value) -> Vec<u8> {
    pack(kind, value.to_string().as_bytes())
}

pub fn screens_json(layout: &Layout) -> serde_json::Value {
    let (w, h) = layout.reach();
    serde_json::json!({"width": w, "height": h, "screens": layout.to_json()})
}

pub fn pong(layout: &Layout) -> Vec<u8> {
    pack_json(PONG, &screens_json(layout))
}

pub fn pong_layout(body: &[u8]) -> Option<Layout> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let (w, h) = (v["width"].as_i64(), v["height"].as_i64());
    (w.is_some() && h.is_some() || v["screens"].is_array()).then(|| Layout::from_json(&v["screens"], w, h))
}

pub fn pack_events(events: &[Event]) -> Vec<u8> {
    let mut body = Vec::with_capacity(events.len() * EVT);
    for &(t, c, v) in events {
        body.push(t);
        body.extend_from_slice(&c.to_be_bytes());
        body.extend_from_slice(&v.to_be_bytes());
    }
    pack(INPUT, &body)
}

pub fn unpack_events(body: &[u8]) -> Vec<Event> {
    body.chunks_exact(EVT)
        .map(|e| (e[0], u16::from_be_bytes([e[1], e[2]]), i32::from_be_bytes([e[3], e[4], e[5], e[6]])))
        .collect()
}

#[derive(Debug)]
pub struct TooBig(pub usize);

impl std::fmt::Display for TooBig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "frame of {} bytes refused", self.0)
    }
}

pub struct Framer {
    buf: Vec<u8>,
    limit: usize,
    hello_only: bool,
}

impl Default for Framer {
    fn default() -> Framer {
        Framer { buf: Vec::new(), limit: MAX_FRAME, hello_only: false }
    }
}

impl Framer {
    pub fn for_hello() -> Framer {
        Framer { buf: Vec::new(), limit: MAX_HELLO, hello_only: true }
    }

    pub fn trusted(mut self) -> (Framer, Vec<(u8, Vec<u8>)>) {
        self.limit = MAX_FRAME;
        self.hello_only = false;
        let rest = self.feed(&[]);
        (self, rest.unwrap_or_default())
    }

    pub fn feed(&mut self, data: &[u8]) -> Result<Vec<(u8, Vec<u8>)>, TooBig> {
        self.buf.extend_from_slice(data);
        let mut out = Vec::new();
        let mut at = 0;
        while self.buf.len() - at >= HDR && (!self.hello_only || out.is_empty()) {
            let kind = self.buf[at];
            let len = u32::from_be_bytes([self.buf[at + 1], self.buf[at + 2], self.buf[at + 3], self.buf[at + 4]]) as usize;
            if len > self.limit {
                return Err(TooBig(len));
            }
            if self.buf.len() - at < HDR + len {
                break;
            }
            out.push((kind, self.buf[at + HDR..at + HDR + len].to_vec()));
            at += HDR + len;
        }
        self.buf.drain(..at);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_in_the_wire_layout() {
        let f = pack_events(&[(2, 0, -5), (1, 0x110, 1)]);
        assert_eq!(&f[..5], &[INPUT, 0, 0, 0, 14]);
        assert_eq!(&f[5..12], &[2, 0, 0, 0xff, 0xff, 0xff, 0xfb]);
        let mut fr = Framer::default();
        let got = fr.feed(&f[..7]).unwrap();
        assert!(got.is_empty());
        let got = fr.feed(&f[7..]).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(unpack_events(&got[0].1), vec![(2, 0, -5), (1, 0x110, 1)]);
    }

    #[test]
    fn several_frames_and_a_refusal() {
        let mut both = pack(PING, b"");
        both.extend(pack_json(HELLO, &serde_json::json!({"id": "a"})));
        let got = Framer::default().feed(&both).unwrap();
        assert_eq!(got[0], (PING, vec![]));
        assert_eq!(got[1].0, HELLO);
        let huge = [CLIPBOARD, 0x7f, 0, 0, 0];
        assert!(Framer::default().feed(&huge).is_err());
    }

    #[test]
    fn a_greeting_may_not_be_large_until_trusted() {
        let big = pack(CLIPBOARD, &vec![b'x'; MAX_HELLO + 1]);
        assert!(Framer::for_hello().feed(&big[..HDR]).is_err());
        let mut both = pack(HELLO, b"{}");
        both.extend(&big);
        let mut fr = Framer::for_hello();
        assert_eq!(fr.feed(&both).unwrap(), vec![(HELLO, b"{}".to_vec())]);
        let (_, rest) = fr.trusted();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].1.len(), MAX_HELLO + 1);
    }
}
