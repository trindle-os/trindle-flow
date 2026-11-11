//! TLS transport: a self-signed certificate pinned by its SHA-256 fingerprint, with the shared token on top.

use std::io::Read;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Instant;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, ServerConfig, ServerConnection, SignatureScheme, StreamOwned};

use crate::config;
use crate::proto::Framer;

pub type ClientStream = StreamOwned<ClientConnection, TcpStream>;
pub type ServerStream = StreamOwned<ServerConnection, TcpStream>;

const VERSIONS: &[&rustls::SupportedProtocolVersion] = &[&rustls::version::TLS13];

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(ring::digest::digest(&ring::digest::SHA256, data).as_ref());
    out
}

pub fn random(bytes: &mut [u8]) -> Result<(), String> {
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), bytes).map_err(|_| "the system gave no randomness".to_string())
}

pub fn fingerprint(der: &[u8]) -> String {
    hex(&sha256(der))
}

#[derive(Debug)]
struct Pinned(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub fn client_config() -> Result<Arc<ClientConfig>, String> {
    let p = provider();
    let cfg = ClientConfig::builder_with_provider(p.clone())
        .with_protocol_versions(VERSIONS)
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned(p)))
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

pub fn tie<D>(conn: &rustls::ConnectionCommon<D>) -> Result<[u8; 32], String> {
    conn.export_keying_material([0u8; 32], b"EXPORTER-flow-pairing", None).map_err(|e| e.to_string())
}

pub fn in_time(sock: &TcpStream, deadline: Instant) -> Result<(), String> {
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err("it took too long to say who it is".into());
    }
    sock.set_read_timeout(Some(left)).map_err(|e| e.to_string())?;
    sock.set_write_timeout(Some(left)).map_err(|e| e.to_string())
}

pub fn read_small<C>(stream: &mut StreamOwned<C, TcpStream>, framer: &mut Framer, deadline: Instant) -> Result<Option<(u8, Vec<u8>)>, String>
where
    StreamOwned<C, TcpStream>: Read,
{
    let mut buf = [0u8; 4096];
    let mut got: &[u8] = &[];
    loop {
        let Ok(frames) = framer.feed(got) else { return Ok(None) };
        if let Some(frame) = frames.into_iter().next() {
            return Ok(Some(frame));
        }
        in_time(&stream.sock, deadline)?;
        let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(None);
        }
        got = &buf[..n];
    }
}

pub fn tune(sock: &TcpStream) {
    let _ = sock.set_nodelay(true);
    let fd = std::os::fd::AsRawFd::as_raw_fd(sock);
    for (level, name, value) in [
        (libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1),
        (libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, 10),
        (libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, 5),
        (libc::IPPROTO_TCP, libc::TCP_KEEPCNT, 3),
    ] {
        let value: libc::c_int = value;
        // SAFETY: an int option on a descriptor `sock` keeps open, read for
        // its size only.
        unsafe { libc::setsockopt(fd, level, name, std::ptr::from_ref(&value).cast(), size_of::<libc::c_int>() as libc::socklen_t) };
    }
}

pub fn connect(host: &str, port: u16, timeout: std::time::Duration) -> Result<(ClientStream, String), String> {
    use std::net::ToSocketAddrs;
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| "no address".to_string())?;
    let sock = TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    tune(&sock);
    sock.set_read_timeout(Some(timeout)).ok();
    sock.set_write_timeout(Some(timeout)).ok();
    let name = ServerName::try_from(crate::discovery::MAGIC).map_err(|e| e.to_string())?.to_owned();
    let conn = ClientConnection::new(client_config()?, name).map_err(|e| e.to_string())?;
    let mut stream = StreamOwned::new(conn, sock);
    while stream.conn.is_handshaking() {
        stream.conn.complete_io(&mut stream.sock).map_err(|e| e.to_string())?;
    }
    let fp = stream
        .conn
        .peer_certificates()
        .and_then(|c| c.first())
        .map(|c| fingerprint(c))
        .ok_or_else(|| "no certificate shown".to_string())?;
    Ok((stream, fp))
}

pub fn ensure_cert() -> Result<String, String> {
    let cert = config::path("cert.pem");
    let key = config::path("key.pem");
    if !(cert.exists() && key.exists()) {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).map_err(|e| e.to_string())?;
        params.distinguished_name.push(rcgen::DnType::CommonName, crate::discovery::MAGIC);
        params.not_before = rcgen::date_time_ymd(2024, 1, 1);
        params.not_after = rcgen::date_time_ymd(2099, 1, 1);
        let pair = rcgen::KeyPair::generate().map_err(|e| e.to_string())?;
        let made = params.self_signed(&pair).map_err(|e| e.to_string())?;
        config::write("key.pem", pair.serialize_pem().as_bytes()).map_err(|e| e.to_string())?;
        config::write("cert.pem", made.pem().as_bytes()).map_err(|e| e.to_string())?;
    }
    let der = CertificateDer::from_pem_file(&cert).map_err(|e| format!("certificate unreadable: {e}"))?;
    Ok(fingerprint(&der))
}

pub fn server_config() -> Result<Arc<ServerConfig>, String> {
    ensure_cert()?;
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(config::path("cert.pem"))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    let key = PrivateKeyDer::from_pem_file(config::path("key.pem")).map_err(|e| format!("key unreadable: {e}"))?;
    let cfg = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(VERSIONS)
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    Ok(Arc::new(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::TestHome;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn a_greeting_sent_a_byte_at_a_time_runs_out_of_time() {
        let _home = TestHome::new();
        let server = server_config().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let mut s = StreamOwned::new(ServerConnection::new(server).unwrap(), sock);
            while s.conn.is_handshaking() {
                s.conn.complete_io(&mut s.sock).unwrap();
            }
            let started = Instant::now();
            let got = read_small(&mut s, &mut Framer::for_hello(), started + std::time::Duration::from_millis(600));
            (got.is_err(), started.elapsed())
        });
        let (mut c, _) = connect("127.0.0.1", port, std::time::Duration::from_secs(5)).unwrap();
        let hello = crate::proto::pack(crate::proto::HELLO, &[b'x'; 40]);
        for byte in &hello {
            if t.is_finished() || c.write_all(&[*byte]).and_then(|()| c.flush()).is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let (cut_off, took) = t.join().unwrap();
        assert!(cut_off, "a trickle never finishing must not be read as a greeting");
        assert!(took < std::time::Duration::from_secs(2), "cut off at the deadline, not per byte: {took:?}");
    }

    #[test]
    fn a_pinned_connection_carries_bytes() {
        let _home = TestHome::new();
        let fp = ensure_cert().unwrap();
        assert_eq!(fp.len(), 64);
        assert_eq!(ensure_cert().unwrap(), fp);
        let server = server_config().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let t = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let conn = ServerConnection::new(server).unwrap();
            let mut s = StreamOwned::new(conn, sock);
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).unwrap();
            s.write_all(&buf).unwrap();
            s.write_all(&tie(&s.conn).unwrap()).unwrap();
            s.flush().unwrap();
        });
        let (mut c, seen) = connect("127.0.0.1", port, std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(seen, fp);
        assert_eq!(c.conn.protocol_version(), Some(rustls::ProtocolVersion::TLSv1_3));
        c.write_all(b"hello").unwrap();
        let mut back = [0u8; 5];
        c.read_exact(&mut back).unwrap();
        assert_eq!(&back, b"hello");
        let mut theirs = [0u8; 32];
        c.read_exact(&mut theirs).unwrap();
        assert_eq!(theirs, tie(&c.conn).unwrap());
        t.join().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = server_config().unwrap();
        let t = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let mut s = StreamOwned::new(ServerConnection::new(server).unwrap(), sock);
            while s.conn.is_handshaking() {
                s.conn.complete_io(&mut s.sock).unwrap();
            }
        });
        let (other, _) = connect("127.0.0.1", port, std::time::Duration::from_secs(5)).unwrap();
        assert_ne!(tie(&other.conn).unwrap(), theirs);
        t.join().unwrap();
    }
}
