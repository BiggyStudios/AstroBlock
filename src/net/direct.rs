//! Playing over an IP address: TCP for what must arrive, UDP for what is only useful if fresh.
//!
//! The host listens on one port number for both. When a client connects over TCP, the host's
//! first frame is a random token; the client then sends that token over UDP, which tells the
//! host where to send its datagrams (and proves who they are from). Until that has worked in
//! both directions, unreliable messages simply take the TCP connection, so a blocked UDP port
//! costs some smoothness and nothing else.
//!
//! Every connection has a reader and a writer thread; the game only touches channels.

use std::{
    collections::hash_map::RandomState,
    hash::{BuildHasher, Hasher},
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Receiver, Sender, unbounded};

use super::{Event, PeerId};
use crate::{net::protocol::MAX_UNRELIABLE, util::FastMap};

/// The port a host listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47_777;
/// Longest single frame: the blob is sent in much smaller pieces, so anything near this is
/// not from a friendly game.
const MAX_FRAME: usize = 8 * 1024 * 1024;
/// How much may wait in a connection's writer before more is held back.
const WRITE_BACKLOG: usize = 2 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// How often the UDP hello is repeated until it is answered.
const UDP_HELLO: Duration = Duration::from_millis(250);

fn random_token() -> u64 {
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u64(Instant::now().elapsed().as_nanos() as u64);
    hasher.finish() | 1
}

/// `host`, `host:port`, or an IPv6 address: the address to connect to.
pub fn resolve(address: &str) -> Result<SocketAddr> {
    let address = address.trim();
    if address.is_empty() {
        bail!("no address given");
    }
    // A bare IP (the colons of an IPv6 address aren't a port).
    if let Ok(ip) = address.parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_PORT));
    }
    let with_port = if address.contains(':') {
        address.to_string()
    } else {
        format!("{address}:{DEFAULT_PORT}")
    };
    with_port
        .to_socket_addrs()
        .with_context(|| format!("can't find {address}"))?
        .next()
        .with_context(|| format!("can't find {address}"))
}

/// Sends frames on a stream from its own thread.
struct Writer {
    frames: Sender<Vec<u8>>,
    /// Bytes handed over and not yet written.
    backlog: Arc<AtomicUsize>,
    stream: TcpStream,
}

impl Writer {
    fn new(stream: TcpStream) -> Result<Self> {
        let (frames, jobs) = unbounded::<Vec<u8>>();
        let backlog = Arc::new(AtomicUsize::new(0));
        let mut out = stream.try_clone()?;
        let pending = backlog.clone();
        std::thread::Builder::new()
            .name("net write".into())
            .spawn(move || {
                while let Ok(frame) = jobs.recv() {
                    let size = (frame.len() as u32).to_le_bytes();
                    let done = out.write_all(&size).and_then(|()| out.write_all(&frame));
                    pending.fetch_sub(frame.len(), Ordering::Relaxed);
                    if done.is_err() {
                        return;
                    }
                }
            })?;
        Ok(Self {
            frames,
            backlog,
            stream,
        })
    }

    /// False if too much is waiting already (try again later).
    fn send(&self, data: Vec<u8>) -> bool {
        if self.backlog.load(Ordering::Relaxed) > WRITE_BACKLOG {
            return false;
        }
        self.backlog.fetch_add(data.len(), Ordering::Relaxed);
        let _ = self.frames.send(data);
        true
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn read_frame(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut size = [0u8; 4];
    stream.read_exact(&mut size).ok()?;
    let size = u32::from_le_bytes(size) as usize;
    if size > MAX_FRAME {
        return None;
    }
    let mut data = vec![0; size];
    stream.read_exact(&mut data).ok()?;
    Some(data)
}

fn with_token(token: u64, data: &[u8]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(8 + data.len());
    packet.extend_from_slice(&token.to_le_bytes());
    packet.extend_from_slice(data);
    packet
}

struct Conn {
    writer: Writer,
    token: u64,
    /// Where its datagrams come from, once one has.
    udp: Arc<Mutex<Option<SocketAddr>>>,
}

type Conns = Arc<Mutex<FastMap<PeerId, Conn>>>;
type Tokens = Arc<Mutex<FastMap<u64, (PeerId, Arc<Mutex<Option<SocketAddr>>>)>>>;

pub struct DirectHost {
    events: Receiver<Event>,
    conns: Conns,
    udp: Arc<UdpSocket>,
    closing: Arc<AtomicBool>,
    pub port: u16,
}

impl DirectHost {
    /// Starts listening on `bind` (`"0.0.0.0:47777"`; port 0 picks a free one).
    pub fn bind(bind: SocketAddr) -> Result<Self> {
        let listener =
            TcpListener::bind(bind).with_context(|| format!("can't listen on {bind}"))?;
        let port = listener.local_addr()?.port();
        let udp = Arc::new(
            UdpSocket::bind(SocketAddr::new(bind.ip(), port))
                .with_context(|| format!("can't listen on UDP port {port}"))?,
        );
        udp.set_read_timeout(Some(Duration::from_millis(200)))?;
        listener.set_nonblocking(true)?;
        let (events_tx, events) = unbounded();
        let conns: Conns = Arc::default();
        let tokens: Tokens = Arc::default();
        let closing = Arc::new(AtomicBool::new(false));
        {
            let (conns, tokens, closing, events) = (
                conns.clone(),
                tokens.clone(),
                closing.clone(),
                events_tx.clone(),
            );
            std::thread::Builder::new()
                .name("net accept".into())
                .spawn(move || accept_loop(listener, conns, tokens, closing, events))?;
        }
        {
            let (udp, closing) = (udp.clone(), closing.clone());
            std::thread::Builder::new()
                .name("net udp".into())
                .spawn(move || udp_host_loop(udp, tokens, closing, events_tx))?;
        }
        Ok(Self {
            events,
            conns,
            udp,
            closing,
            port,
        })
    }

    pub fn poll(&mut self, out: &mut Vec<Event>) {
        out.extend(self.events.try_iter());
    }

    /// False if a reliable message has to wait.
    pub fn send(&mut self, peer: PeerId, data: &[u8], reliable: bool) -> bool {
        let conns = self.conns.lock().expect("conns");
        let Some(conn) = conns.get(&peer) else {
            return true; // gone: nothing will ever take it
        };
        if !reliable && data.len() <= MAX_UNRELIABLE {
            let addr = *conn.udp.lock().expect("udp");
            if let Some(addr) = addr {
                let _ = self.udp.send_to(&with_token(conn.token, data), addr);
                return true;
            }
        }
        conn.writer.send(data.to_vec())
    }

    pub fn close(&mut self, peer: PeerId) {
        self.conns.lock().expect("conns").remove(&peer);
    }
}

impl Drop for DirectHost {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Relaxed);
        self.conns.lock().expect("conns").clear();
    }
}

fn accept_loop(
    listener: TcpListener,
    conns: Conns,
    tokens: Tokens,
    closing: Arc<AtomicBool>,
    events: Sender<Event>,
) {
    let mut next = 0u32;
    while !closing.load(Ordering::Relaxed) {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(_) => {
                std::thread::sleep(Duration::from_millis(30));
                continue;
            }
        };
        next += 1;
        let peer = PeerId::direct(next);
        if let Err(e) = add_peer(stream, peer, &conns, &tokens, &events) {
            log::warn!("a connection failed: {e:#}");
        }
    }
}

fn add_peer(
    stream: TcpStream,
    peer: PeerId,
    conns: &Conns,
    tokens: &Tokens,
    events: &Sender<Event>,
) -> Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    let mut reader = stream.try_clone()?;
    let writer = Writer::new(stream)?;
    let token = random_token();
    let udp = Arc::new(Mutex::new(None));
    writer.send(token.to_le_bytes().to_vec());
    tokens
        .lock()
        .expect("tokens")
        .insert(token, (peer, udp.clone()));
    conns
        .lock()
        .expect("conns")
        .insert(peer, Conn { writer, token, udp });
    let _ = events.send(Event::Connected(peer));
    let (events, conns, tokens) = (events.clone(), conns.clone(), tokens.clone());
    std::thread::Builder::new()
        .name("net read".into())
        .spawn(move || {
            while let Some(frame) = read_frame(&mut reader) {
                if events.send(Event::Received(peer, frame)).is_err() {
                    return;
                }
            }
            conns.lock().expect("conns").remove(&peer);
            tokens.lock().expect("tokens").remove(&token);
            let _ = events.send(Event::Lost(peer, "connection lost".to_string()));
        })?;
    Ok(())
}

fn udp_host_loop(
    udp: Arc<UdpSocket>,
    tokens: Tokens,
    closing: Arc<AtomicBool>,
    events: Sender<Event>,
) {
    let mut buffer = [0u8; 2048];
    while !closing.load(Ordering::Relaxed) {
        let Ok((n, from)) = udp.recv_from(&mut buffer) else {
            continue;
        };
        let Some(token) = buffer.get(..8).filter(|_| n >= 8) else {
            continue;
        };
        let token = u64::from_le_bytes(token.try_into().expect("8 bytes"));
        let Some((peer, addr)) = tokens.lock().expect("tokens").get(&token).cloned() else {
            continue; // not one of ours
        };
        *addr.lock().expect("udp") = Some(from);
        if n == 8 {
            let _ = udp.send_to(&token.to_le_bytes(), from); // hello: answer it
        } else {
            let _ = events.send(Event::Received(peer, buffer[8..n].to_vec()));
        }
    }
}

struct Connected {
    writer: Writer,
    token: u64,
    udp: Arc<UdpSocket>,
    udp_works: Arc<AtomicBool>,
    last_hello: Instant,
}

pub struct DirectClient {
    events: Receiver<Event>,
    tx: Sender<Event>,
    ready: Receiver<Connected>,
    conn: Option<Connected>,
    closing: Arc<AtomicBool>,
}

impl DirectClient {
    /// Starts connecting; `Event::Connected` or `Event::Lost` tells how it went.
    pub fn connect(address: SocketAddr) -> Self {
        let (tx, events) = unbounded();
        let (ready_tx, ready) = unbounded();
        let closing = Arc::new(AtomicBool::new(false));
        let (events_tx, flag) = (tx.clone(), closing.clone());
        let thread = std::thread::Builder::new()
            .name("net connect".into())
            .spawn(move || {
                if let Err(e) = client_thread(address, &events_tx, &ready_tx, &flag) {
                    let _ = events_tx.send(Event::Lost(PeerId::HOST, format!("{e:#}")));
                }
            });
        if thread.is_err() {
            let _ = tx.send(Event::Lost(PeerId::HOST, "out of threads".to_string()));
        }
        Self {
            events,
            tx,
            ready,
            conn: None,
            closing,
        }
    }

    pub fn poll(&mut self, out: &mut Vec<Event>) {
        if let Ok(conn) = self.ready.try_recv() {
            self.conn = Some(conn);
            let _ = self.tx.send(Event::Connected(PeerId::HOST));
        }
        if let Some(conn) = &mut self.conn
            && !conn.udp_works.load(Ordering::Relaxed)
            && conn.last_hello.elapsed() > UDP_HELLO
        {
            conn.last_hello = Instant::now();
            let _ = conn.udp.send(&conn.token.to_le_bytes());
        }
        out.extend(self.events.try_iter());
    }

    pub fn send(&mut self, data: &[u8], reliable: bool) -> bool {
        let Some(conn) = &self.conn else {
            return false;
        };
        if !reliable && data.len() <= MAX_UNRELIABLE && conn.udp_works.load(Ordering::Relaxed) {
            let _ = conn.udp.send(&with_token(conn.token, data));
            return true;
        }
        conn.writer.send(data.to_vec())
    }
}

impl Drop for DirectClient {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Relaxed);
    }
}

fn client_thread(
    address: SocketAddr,
    events: &Sender<Event>,
    ready: &Sender<Connected>,
    closing: &Arc<AtomicBool>,
) -> Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
        .with_context(|| format!("nobody answered at {address}"))?;
    stream.set_nodelay(true)?;
    // The first frame is our token.
    let token = read_frame(&mut stream).context("the host hung up")?;
    let token = u64::from_le_bytes(
        token
            .as_slice()
            .try_into()
            .context("not an astro block host")?,
    );
    let local: SocketAddr = if address.is_ipv4() {
        "0.0.0.0:0".parse()?
    } else {
        "[::]:0".parse()?
    };
    let udp = Arc::new(UdpSocket::bind(local)?);
    udp.connect(address)?;
    udp.set_read_timeout(Some(Duration::from_millis(200)))?;
    let udp_works = Arc::new(AtomicBool::new(false));
    {
        let (udp, works, closing, events) = (
            udp.clone(),
            udp_works.clone(),
            closing.clone(),
            events.clone(),
        );
        std::thread::Builder::new()
            .name("net udp".into())
            .spawn(move || {
                let mut buffer = [0u8; 2048];
                while !closing.load(Ordering::Relaxed) {
                    let Ok(n) = udp.recv(&mut buffer) else {
                        continue;
                    };
                    if n < 8 || buffer[..8] != token.to_le_bytes() {
                        continue;
                    }
                    works.store(true, Ordering::Relaxed);
                    if n > 8 {
                        let _ = events.send(Event::Received(PeerId::HOST, buffer[8..n].to_vec()));
                    }
                }
            })?;
    }
    let writer = Writer::new(stream.try_clone()?)?;
    let _ = ready.send(Connected {
        writer,
        token,
        udp,
        udp_works,
        last_hello: Instant::now() - UDP_HELLO,
    });
    while let Some(frame) = read_frame(&mut stream) {
        if events.send(Event::Received(PeerId::HOST, frame)).is_err() {
            return Ok(());
        }
    }
    let _ = events.send(Event::Lost(PeerId::HOST, "the host hung up".to_string()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Polls `f` until it says it is done (or fails after a few seconds).
    fn wait(what: &str, mut f: impl FnMut() -> bool) {
        let start = Instant::now();
        while !f() {
            assert!(
                start.elapsed() < Duration::from_secs(6),
                "timed out: {what}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn addresses_get_the_default_port() {
        assert_eq!(resolve("127.0.0.1").unwrap().port(), DEFAULT_PORT);
        assert_eq!(resolve("127.0.0.1:5000").unwrap().port(), 5000);
        assert_eq!(resolve("::1").unwrap().port(), DEFAULT_PORT);
        assert_eq!(resolve("[::1]:5000").unwrap().port(), 5000);
        assert!(resolve("  ").is_err());
    }

    #[test]
    fn host_and_client_talk_both_ways_over_tcp_and_udp() {
        let bind = "127.0.0.1:0".parse().unwrap();
        let mut host = DirectHost::bind(bind).unwrap();
        let mut client =
            DirectClient::connect(SocketAddr::new("127.0.0.1".parse().unwrap(), host.port));
        let (mut at_host, mut at_client) = (Vec::new(), Vec::new());
        let mut peer = None;
        wait("connecting", || {
            host.poll(&mut at_host);
            client.poll(&mut at_client);
            for e in at_host.drain(..) {
                if let Event::Connected(p) = e {
                    peer = Some(p);
                }
            }
            peer.is_some() && at_client.iter().any(|e| matches!(e, Event::Connected(_)))
        });
        let peer = peer.unwrap();
        at_client.clear();

        // Reliable, in order, and bigger than any datagram.
        let big = vec![42u8; 300_000];
        assert!(client.send(&big, true));
        assert!(host.send(peer, b"from the host", true));
        let mut got_big = false;
        let mut got_small = false;
        wait("reliable messages", || {
            host.poll(&mut at_host);
            client.poll(&mut at_client);
            got_big |= at_host
                .iter()
                .any(|e| matches!(e, Event::Received(_, d) if d == &big));
            got_small |= at_client
                .iter()
                .any(|e| matches!(e, Event::Received(_, d) if d == b"from the host"));
            got_big && got_small
        });
        at_host.clear();
        at_client.clear();

        // UDP gets going by itself; the messages arrive one way or another.
        wait("udp", || {
            host.poll(&mut at_host);
            client.poll(&mut at_client);
            assert!(host.send(peer, b"datagram", false));
            at_client
                .iter()
                .any(|e| matches!(e, Event::Received(_, d) if d == b"datagram"))
        });
        wait("udp the other way", || {
            host.poll(&mut at_host);
            client.poll(&mut at_client);
            assert!(client.send(b"hello there", false));
            at_host
                .iter()
                .any(|e| matches!(e, Event::Received(_, d) if d == b"hello there"))
        });

        // Hanging up is noticed.
        drop(client);
        wait("hanging up", || {
            host.poll(&mut at_host);
            at_host
                .iter()
                .any(|e| matches!(e, Event::Lost(p, _) if *p == peer))
        });
    }

    #[test]
    fn nobody_home_is_reported() {
        // A port that was just free.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let mut client = DirectClient::connect(SocketAddr::new("127.0.0.1".parse().unwrap(), port));
        let mut events = Vec::new();
        let start = Instant::now();
        while !events.iter().any(|e| matches!(e, Event::Lost(..))) {
            assert!(start.elapsed() < Duration::from_secs(10), "no answer");
            client.poll(&mut events);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn huge_frames_are_refused() {
        let mut host = DirectHost::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut raw = TcpStream::connect(("127.0.0.1", host.port)).unwrap();
        raw.write_all(&u32::MAX.to_le_bytes()).unwrap();
        let mut events = Vec::new();
        wait("the bad connection to be dropped", || {
            host.poll(&mut events);
            events.iter().any(|e| matches!(e, Event::Lost(..)))
        });
    }
}
