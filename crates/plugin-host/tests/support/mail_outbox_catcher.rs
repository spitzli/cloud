//! Bounded, plain loopback SMTP fault fixture. Captures never leave private memory.
//! `stored` means complete DATA retained by this catcher, not a Core receipt claim.
use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::Notify,
    task::{JoinHandle, JoinSet},
    time::timeout,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Accept,
    RejectBeforeData,
    RejectAfterData,
    DisconnectBeforeData,
    StoreThenDropFinalReply,
    BlockFinalReply,
}
#[derive(Clone, Copy)]
pub struct Limits {
    pub line_bytes: usize,
    pub body_bytes: usize,
    /// Global DATA reservations, consumed even by rejected or abandoned attempts.
    pub messages: usize,
    pub recipients: usize,
    pub commands: usize,
    pub connections: usize,
    pub lifetime: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            line_bytes: 4096,
            body_bytes: 1024 * 1024,
            messages: 16,
            recipients: 100,
            commands: 128,
            connections: 32,
            lifetime: Duration::from_secs(5),
        }
    }
}
#[derive(Clone, Copy, Default, Debug)]
pub struct Counts {
    /// Every observed DATA command, including invalid envelope or exhausted budget.
    pub data_attempts: usize,
    /// Complete dot-terminated DATA, including definitive post-DATA rejection.
    pub complete_data: usize,
    /// Complete DATA retained in memory; excludes both explicit rejection modes.
    pub stored: usize,
    pub active: usize,
    pub blocked: usize,
}
struct Capture {
    from: String,
    to: Vec<String>,
    mime: Vec<u8>,
}
#[derive(Default)]
struct State {
    counts: Counts,
    data_slots: usize,
    captures: Vec<Capture>,
}
pub struct Catcher {
    address: SocketAddr,
    state: Arc<Mutex<State>>,
    release: Arc<Notify>,
    task: JoinHandle<()>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    closed: bool,
}
impl Catcher {
    pub async fn start(mode: Mode, limits: Limits) -> io::Result<Self> {
        if !(8..=65536).contains(&limits.line_bytes)
            || !(1..=16 * 1024 * 1024).contains(&limits.body_bytes)
            || !(1..=128).contains(&limits.messages)
            || !(1..=100).contains(&limits.recipients)
            || !(1..=1024).contains(&limits.commands)
            || !(1..=128).contains(&limits.connections)
            || limits.lifetime.is_zero()
            || limits.lifetime > Duration::from_secs(30)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid catcher limits",
            ));
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let state = Arc::new(Mutex::new(State::default()));
        let release = Arc::new(Notify::new());
        let shared = state.clone();
        let gate = release.clone();
        let (stop, mut stopped) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            // JoinSet aborts every child if the owner aborts this listener task.
            let mut connections = JoinSet::new();
            let mut accepted = 0;
            let mut closing = false;
            while accepted < limits.connections {
                tokio::select! {
                    _=&mut stopped=>{closing=true;break;}
                    connection = listener.accept() => {
                        let Ok((stream, peer)) = connection else { break };
                        if !peer.ip().is_loopback() { continue; }
                        accepted += 1;
                        let state = shared.clone();
                        let release = gate.clone();
                        connections.spawn(async move {
                            let _active = Counter::new(state.clone(), false);
                            let _ = timeout(limits.lifetime, serve(stream, mode, limits, state, release)).await;
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
            drop(listener);
            if closing {
                connections.shutdown().await;
            } else {
                tokio::select! {
                    _=&mut stopped=>connections.shutdown().await,
                    _=async {while connections.join_next().await.is_some(){}}=>{}
                }
            }
        });
        Ok(Self {
            address,
            state,
            release,
            task,
            stop: Some(stop),
            closed: false,
        })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn counts(&self) -> Counts {
        self.state.lock().unwrap().counts
    }
    /// Equality only: no Debug, raw body, envelope or credential accessors.
    pub fn matches(&self, index: usize, from: &str, to: &[&str], mime: &[u8]) -> bool {
        self.state
            .lock()
            .unwrap()
            .captures
            .get(index)
            .is_some_and(|capture| capture.from == from && capture.to == to && capture.mime == mime)
    }
    /// Compare actual signed Core MIME commitment without exposing generated Date/Message-ID.
    pub fn matches_commitment(
        &self,
        index: usize,
        from: &str,
        to: &[&str],
        digest: &str,
        bytes: usize,
    ) -> bool {
        use sha2::{Digest, Sha256};
        self.state
            .lock()
            .unwrap()
            .captures
            .get(index)
            .is_some_and(|capture| {
                capture.from == from
                    && capture.to == to
                    && capture.mime.len() == bytes
                    && hex::encode(Sha256::digest(&capture.mime)) == digest
            })
    }
    /// One bounded notification; intended to release one observed blocked attempt.
    /// Exact existing mail-send framing: logical prepared MIME plus ONE CRLF before dot.
    /// Keep full stored DATA unchanged; no trimming, fallback, normalization or suffix union.
    pub fn matches_smtp_framed_commitment(
        &self,
        index: usize,
        from: &str,
        to: &[&str],
        digest: &str,
        prepared_bytes: usize,
    ) -> bool {
        use sha2::{Digest, Sha256};
        let Some(framed_bytes) = prepared_bytes.checked_add(2) else {
            return false;
        };
        self.state
            .lock()
            .unwrap()
            .captures
            .get(index)
            .is_some_and(|capture| {
                capture.from == from
                    && capture.to == to
                    && capture.mime.len() == framed_bytes
                    && capture.mime.get(prepared_bytes..) == Some(&b"\r\n"[..])
                    && capture
                        .mime
                        .get(..prepared_bytes)
                        .is_some_and(|prepared| hex::encode(Sha256::digest(prepared)) == digest)
            })
    }
    pub fn release_final_reply(&self) {
        self.release.notify_one();
    }
    /// Cancel the owned listener and its child tasks, then await listener exit.
    pub async fn close(&mut self) {
        if self.closed {
            return;
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = (&mut self.task).await;
        let mut state = self.state.lock().unwrap();
        for capture in &mut state.captures {
            capture.mime.fill(0);
            capture
                .from
                .replace_range(.., &"\0".repeat(capture.from.len()));
            for recipient in &mut capture.to {
                recipient.replace_range(.., &"\0".repeat(recipient.len()));
            }
        }
        state.captures.clear();
        self.closed = true;
    }
    pub async fn shutdown(mut self) {
        self.close().await;
    }
}
impl Drop for Catcher {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct Counter {
    state: Arc<Mutex<State>>,
    blocked: bool,
}
impl Counter {
    fn new(state: Arc<Mutex<State>>, blocked: bool) -> Self {
        {
            let counts = &mut state.lock().unwrap().counts;
            if blocked {
                counts.blocked += 1;
            } else {
                counts.active += 1;
            }
        }
        Self { state, blocked }
    }
}
impl Drop for Counter {
    fn drop(&mut self) {
        let counts = &mut self.state.lock().unwrap().counts;
        if self.blocked {
            counts.blocked -= 1;
        } else {
            counts.active -= 1;
        }
    }
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "invalid or oversized SMTP fixture input",
    )
}

// Never use read_until: an untrusted line without LF must not grow an allocation.
async fn line(reader: &mut BufReader<TcpStream>, limit: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Err(invalid());
        }
        let count = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |n| n + 1);
        if output.len() + count > limit {
            return Err(invalid());
        }
        output.extend_from_slice(&available[..count]);
        reader.consume(count);
        if output.ends_with(b"\n") {
            if !output.ends_with(b"\r\n")
                || output[..output.len() - 2]
                    .iter()
                    .any(|b| matches!(b, b'\r' | b'\n' | 0))
            {
                return Err(invalid());
            }
            return Ok(output);
        }
    }
}

async fn reply(reader: &mut BufReader<TcpStream>, bytes: &[u8]) -> io::Result<()> {
    reader.get_mut().write_all(bytes).await
}

fn address(input: &str, prefix: &str, parameters: bool) -> Option<String> {
    let rest = input
        .get(..prefix.len())
        .filter(|v| v.eq_ignore_ascii_case(prefix))
        .and_then(|_| input.get(prefix.len()..))?;
    let rest = rest.strip_prefix('<')?;
    let (address, suffix) = rest.split_once('>')?;
    if address.is_empty()
        || address
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, '<' | '>'))
    {
        return None;
    }
    if !suffix.is_empty()
        && (!parameters
            || !suffix
                .split(' ')
                .skip(1)
                .all(|s| matches!(s, "SMTPUTF8" | "BODY=8BITMIME"))
            || !suffix.starts_with(' '))
    {
        return None;
    }
    Some(address.to_owned())
}

async fn serve(
    stream: TcpStream,
    mode: Mode,
    limits: Limits,
    state: Arc<Mutex<State>>,
    release: Arc<Notify>,
) -> io::Result<()> {
    let mut reader = BufReader::new(stream);
    reply(&mut reader, b"220 loopback fixture\r\n").await?;
    let mut greeted = false;
    let mut from = None;
    let mut to = Vec::new();
    for _ in 0..limits.commands {
        let command = line(&mut reader, limits.line_bytes).await?;
        let command = std::str::from_utf8(&command[..command.len() - 2]).map_err(|_| invalid())?;
        if command
            .get(..5)
            .is_some_and(|c| c.eq_ignore_ascii_case("EHLO ") || c.eq_ignore_ascii_case("HELO "))
        {
            greeted = true;
            from = None;
            to.clear();
            reply(
                &mut reader,
                b"250-loopback fixture\r\n250-8BITMIME\r\n250 SMTPUTF8\r\n",
            )
            .await?;
        } else if command.eq_ignore_ascii_case("QUIT") {
            reply(&mut reader, b"221 bye\r\n").await?;
            return Ok(());
        } else if command.eq_ignore_ascii_case("RSET") {
            from = None;
            to.clear();
            reply(&mut reader, b"250 reset\r\n").await?;
        } else if command.eq_ignore_ascii_case("NOOP") {
            reply(&mut reader, b"250 ok\r\n").await?;
        } else if let Some(sender) = address(command, "MAIL FROM:", true).filter(|_| greeted) {
            from = Some(sender);
            to.clear();
            reply(&mut reader, b"250 sender\r\n").await?;
        } else if let Some(recipient) =
            address(command, "RCPT TO:", false).filter(|_| from.is_some())
        {
            if to.len() >= limits.recipients {
                reply(&mut reader, b"452 recipient limit\r\n").await?;
            } else {
                to.push(recipient);
                reply(&mut reader, b"250 recipient\r\n").await?;
            }
        } else if command.eq_ignore_ascii_case("DATA") {
            state.lock().unwrap().counts.data_attempts += 1;
            if from.is_none() || to.is_empty() {
                reply(&mut reader, b"503 envelope required\r\n").await?;
                continue;
            }
            let within_budget = {
                let mut state = state.lock().unwrap();
                if state.data_slots >= limits.messages {
                    false
                } else {
                    state.data_slots += 1;
                    true
                }
            };
            if !within_budget {
                reply(&mut reader, b"452 DATA limit\r\n").await?;
                continue;
            }
            if mode == Mode::DisconnectBeforeData {
                return Ok(());
            }
            if mode == Mode::RejectBeforeData {
                reply(&mut reader, b"550 rejected before DATA\r\n").await?;
                from = None;
                to.clear();
                continue;
            }
            reply(&mut reader, b"354 send DATA\r\n").await?;
            let mut mime = Vec::new();
            loop {
                let bytes = line(&mut reader, limits.line_bytes).await?;
                if bytes == b".\r\n" {
                    break;
                }
                let bytes = if bytes.starts_with(b".") {
                    if !bytes.starts_with(b"..") {
                        return Err(invalid());
                    }
                    &bytes[1..]
                } else {
                    &bytes[..]
                };
                if mime.len() + bytes.len() > limits.body_bytes {
                    return Err(invalid());
                }
                mime.extend_from_slice(bytes);
            }
            {
                let mut state = state.lock().unwrap();
                state.counts.complete_data += 1;
                if mode != Mode::RejectAfterData {
                    state.captures.push(Capture {
                        from: from.take().ok_or_else(invalid)?,
                        to: std::mem::take(&mut to),
                        mime,
                    });
                    state.counts.stored += 1;
                }
            }
            from = None;
            to.clear();
            match mode {
                Mode::StoreThenDropFinalReply => return Ok(()),
                Mode::RejectAfterData => reply(&mut reader, b"550 rejected after DATA\r\n").await?,
                Mode::BlockFinalReply => {
                    let _blocked = Counter::new(state.clone(), true);
                    release.notified().await;
                    reply(&mut reader, b"250 stored\r\n").await?;
                }
                _ => reply(&mut reader, b"250 stored\r\n").await?,
            }
        } else {
            reply(&mut reader, b"503 unsupported command or sequence\r\n").await?;
        }
    }
    Ok(())
}
