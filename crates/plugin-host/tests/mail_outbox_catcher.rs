#[path = "support/mail_outbox_catcher.rs"]
mod catcher;

use catcher::{Catcher, Counts, Limits, Mode};
use sha2::Digest;
use std::{io, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpStream,
    time::{sleep, timeout},
};

const MIME: &[u8] = b"Subject: fixture\r\n\r\nfirst\r\n.dot\r\n..two\r\n";
const WIRE: &[u8] = b"Subject: fixture\r\n\r\nfirst\r\n..dot\r\n...two\r\n.\r\n";
const DEADLINE: Duration = Duration::from_secs(2);

#[tokio::test]
async fn actual_smtp_adapter_preserves_prepared_commitment_with_exact_framing() {
    for raw in [MIME, MIME.strip_suffix(b"\r\n").unwrap()] {
        for mode in [Mode::Accept, Mode::StoreThenDropFinalReply] {
            let catcher = Catcher::start(mode, Limits::default()).await.unwrap();
            let result = webdock_mail::smtp::submit_loopback(
                "127.0.0.1",
                catcher.address().port(),
                "sender@example.test",
                &["one@example.test".into(), "two@example.test".into()],
                raw,
                DEADLINE,
            )
            .await;
            if mode == Mode::Accept {
                assert!(result.is_ok(), "owned SMTP submit failed");
            } else {
                assert!(
                    matches!(result, Err(error) if error.uncertain),
                    "final reply loss was not uncertain"
                );
            }
            let digest = hex::encode(sha2::Sha256::digest(raw));
            assert!(
                catcher.matches_smtp_framed_commitment(
                    0,
                    "sender@example.test",
                    &["one@example.test", "two@example.test"],
                    &digest,
                    raw.len()
                ),
                "strict framed commitment did not match"
            );
            assert!(
                !catcher.matches_commitment(
                    0,
                    "sender@example.test",
                    &["one@example.test", "two@example.test"],
                    &digest,
                    raw.len()
                ),
                "generic exact matcher silently accepted framing"
            );
            let framed = [raw, b"\r\n"].concat();
            assert!(
                catcher.matches(
                    0,
                    "sender@example.test",
                    &["one@example.test", "two@example.test"],
                    &framed
                ),
                "full DATA was changed or discarded"
            );
            let count = catcher.counts();
            assert!(
                count.data_attempts == 1 && count.complete_data == 1 && count.stored == 1,
                "SMTP framing changed full DATA counts"
            );
            catcher.shutdown().await;
        }
    }
}

#[tokio::test]
async fn smtp_framed_matcher_rejects_changed_or_missing_transport_bytes() {
    let prepared = &b"Subject: boundary\r\n\r\nbody\r\n"[..];
    let digest = hex::encode(sha2::Sha256::digest(prepared));
    let examples = [
        ([prepared, b"\r\n"].concat(), true, false),
        (prepared.to_vec(), false, true),
        ([prepared, b"\r\n\r\n"].concat(), false, false),
        (
            [&b"Subject: boundary\r\n\r\nBody\r\n"[..], b"\r\n"].concat(),
            false,
            false,
        ),
        ([prepared, b"XX\r\n"].concat(), false, false),
    ];
    for (data, framed_match, exact_match) in examples {
        let catcher = Catcher::start(Mode::Accept, Limits::default())
            .await
            .unwrap();
        let mut client = Client::connect(&catcher).await;
        client.envelope().await;
        client.write(b"DATA\r\n").await;
        assert!(
            client.reply().await == Some(354),
            "owned DATA admission failed"
        );
        client.write(&[data.as_slice(), b".\r\n"].concat()).await;
        assert!(
            client.reply().await == Some(250),
            "owned DATA storage failed"
        );
        assert!(
            catcher.matches_smtp_framed_commitment(
                0,
                "sender@example.test",
                &["one@example.test", "two@example.test"],
                &digest,
                prepared.len()
            ) == framed_match,
            "strict framing predicate accepted invalid DATA or rejected exact framing"
        );
        assert!(
            catcher.matches_commitment(
                0,
                "sender@example.test",
                &["one@example.test", "two@example.test"],
                &digest,
                prepared.len()
            ) == exact_match,
            "generic exact commitment behavior changed"
        );
        assert!(
            !catcher.matches_smtp_framed_commitment(
                0,
                "wrong@example.test",
                &["one@example.test", "two@example.test"],
                &digest,
                prepared.len()
            ),
            "wrong sender passed"
        );
        assert!(
            !catcher.matches_smtp_framed_commitment(
                0,
                "sender@example.test",
                &["one@example.test"],
                &digest,
                prepared.len()
            ),
            "wrong envelope passed"
        );
        for bytes in [prepared.len() - 1, prepared.len() + 1, usize::MAX] {
            assert!(
                !catcher.matches_smtp_framed_commitment(
                    0,
                    "sender@example.test",
                    &["one@example.test", "two@example.test"],
                    &digest,
                    bytes
                ),
                "wrong or overflowing prepared length passed"
            );
        }
        assert!(
            !catcher.matches_smtp_framed_commitment(
                0,
                "sender@example.test",
                &["one@example.test", "two@example.test"],
                &"0".repeat(64),
                prepared.len()
            ),
            "wrong digest passed"
        );
        assert!(
            catcher.matches(
                0,
                "sender@example.test",
                &["one@example.test", "two@example.test"],
                &data
            ),
            "framing query mutated full stored DATA"
        );
        let count = catcher.counts();
        assert!(
            count.data_attempts == 1 && count.complete_data == 1 && count.stored == 1,
            "negative predicate changed DATA counts"
        );
        catcher.shutdown().await;
    }
}

struct Client(BufReader<TcpStream>);
impl Client {
    async fn connect(catcher: &Catcher) -> Self {
        let stream = timeout(DEADLINE, TcpStream::connect(catcher.address()))
            .await
            .expect("owned connection deadline")
            .expect("owned connection failed");
        let mut client = Self(BufReader::new(stream));
        assert!(client.reply().await == Some(220), "missing SMTP greeting");
        client
    }
    async fn write(&mut self, bytes: &[u8]) {
        timeout(DEADLINE, self.0.get_mut().write_all(bytes))
            .await
            .expect("owned write deadline")
            .expect("owned write failed");
    }
    async fn reply(&mut self) -> Option<u16> {
        timeout(DEADLINE, async {
            loop {
                let mut line = Vec::new();
                // Bound even the independent raw client; never display reply contents.
                for _ in 0..512 {
                    let bytes = self.0.fill_buf().await.ok()?;
                    let byte = *bytes.first()?;
                    self.0.consume(1);
                    line.push(byte);
                    if byte == b'\n' {
                        break;
                    }
                }
                if !line.ends_with(b"\r\n") || line.len() < 6 {
                    return None;
                }
                let code = std::str::from_utf8(&line[..3]).ok()?.parse().ok()?;
                match line[3] {
                    b' ' => return Some(code),
                    b'-' => {}
                    _ => return None,
                }
            }
        })
        .await
        .expect("owned reply deadline")
    }
    async fn envelope(&mut self) {
        for command in [
            &b"EHLO fixture.invalid\r\n"[..],
            b"MAIL FROM:<sender@example.test> SMTPUTF8 BODY=8BITMIME\r\n",
            b"RCPT TO:<one@example.test>\r\n",
            b"RCPT TO:<two@example.test>\r\n",
        ] {
            self.write(command).await;
            assert!(self.reply().await == Some(250), "envelope refused");
        }
    }
    async fn data(&mut self) {
        self.write(b"DATA\r\n").await;
        assert!(self.reply().await == Some(354), "DATA not admitted");
        // Fragment CRLF and dot transparency across TCP writes.
        for chunk in WIRE.chunks(3) {
            self.write(chunk).await;
        }
    }
}

async fn observed(catcher: &Catcher, ready: impl Fn(Counts) -> bool) {
    timeout(DEADLINE, async {
        while !ready(catcher.counts()) {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("owned observation deadline");
}

fn captured(catcher: &Catcher, index: usize) -> bool {
    catcher.matches(
        index,
        "sender@example.test",
        &["one@example.test", "two@example.test"],
        MIME,
    )
}

// Catches early/truncated capture, dot transparency errors and envelope loss.
#[tokio::test]
async fn acceptance_captures_complete_data_and_resets_transaction() {
    let catcher = Catcher::start(Mode::Accept, Limits::default())
        .await
        .unwrap();
    let mut client = Client::connect(&catcher).await;
    for _ in 0..2 {
        client.envelope().await;
        client.data().await;
        assert!(client.reply().await == Some(250), "final acceptance absent");
    }
    let counts = catcher.counts();
    assert_eq!(counts.data_attempts, 2);
    assert_eq!(counts.complete_data, 2);
    assert_eq!(counts.stored, 2);
    assert!(
        captured(&catcher, 0) && captured(&catcher, 1),
        "capture mismatch"
    );
    let digest = hex::encode(sha2::Sha256::digest(MIME));
    assert!(catcher.matches_commitment(
        0,
        "sender@example.test",
        &["one@example.test", "two@example.test"],
        &digest,
        MIME.len()
    ));
    assert!(!catcher.matches_commitment(
        0,
        "sender@example.test",
        &["one@example.test", "two@example.test"],
        &digest,
        MIME.len() + 1
    ));
    assert!(!catcher.matches_commitment(
        0,
        "sender@example.test",
        &["one@example.test", "two@example.test"],
        &"0".repeat(64),
        MIME.len()
    ));
    assert!(!catcher.matches(0, "wrong@example.test", &[], MIME));
    assert!(!catcher.matches(0, "sender@example.test", &["one@example.test"], MIME));
    assert!(!catcher.matches(
        0,
        "sender@example.test",
        &["one@example.test", "two@example.test"],
        b"different"
    ));
    assert!(!captured(&catcher, 2));
    client.write(b"QUIT\r\n").await;
    assert!(client.reply().await == Some(221), "QUIT failed");
    catcher.shutdown().await;
}

// Catches definitive rejection or pre-DATA disconnect being counted as accepted.
#[tokio::test]
async fn explicit_rejection_and_disconnect_never_count_as_stored() {
    for mode in [
        Mode::RejectBeforeData,
        Mode::RejectAfterData,
        Mode::DisconnectBeforeData,
    ] {
        let catcher = Catcher::start(mode, Limits::default()).await.unwrap();
        let mut client = Client::connect(&catcher).await;
        client.envelope().await;
        if mode == Mode::RejectAfterData {
            client.data().await;
            assert!(client.reply().await == Some(550), "post-DATA reject absent");
        } else {
            client.write(b"DATA\r\n").await;
            let want = if mode == Mode::RejectBeforeData {
                Some(550)
            } else {
                None
            };
            assert!(client.reply().await == want, "pre-DATA fault absent");
        }
        let counts = catcher.counts();
        assert_eq!(counts.data_attempts, 1);
        assert_eq!(
            counts.complete_data,
            usize::from(mode == Mode::RejectAfterData)
        );
        assert_eq!(counts.stored, 0);
        assert!(!captured(&catcher, 0));
        catcher.shutdown().await;
    }
}

// Catches the critical false proof: disconnecting without storing complete DATA.
#[tokio::test]
async fn drop_final_reply_stores_full_message_before_eof() {
    let catcher = Catcher::start(Mode::StoreThenDropFinalReply, Limits::default())
        .await
        .unwrap();
    let mut client = Client::connect(&catcher).await;
    client.envelope().await;
    client.data().await;
    assert!(
        client.reply().await.is_none(),
        "final reply was not dropped"
    );
    assert_eq!(catcher.counts().complete_data, 1);
    assert_eq!(catcher.counts().stored, 1);
    assert!(captured(&catcher, 0), "full DATA not privately stored");
    catcher.shutdown().await;
}

// Catches a race gate replying early or storing only after release.
#[tokio::test]
async fn blocked_final_reply_is_observable_and_released_explicitly() {
    let catcher = Catcher::start(Mode::BlockFinalReply, Limits::default())
        .await
        .unwrap();
    let mut client = Client::connect(&catcher).await;
    client.envelope().await;
    client.data().await;
    observed(&catcher, |c| c.blocked == 1).await;
    assert!(captured(&catcher, 0), "blocked DATA not yet stored");
    assert!(
        timeout(Duration::from_millis(30), client.reply())
            .await
            .is_err(),
        "reply escaped gate"
    );
    catcher.release_final_reply();
    assert!(client.reply().await == Some(250), "release did not accept");
    observed(&catcher, |c| c.blocked == 0).await;
    catcher.shutdown().await;
}

// Catches unbounded peer lifetime, including a final reply never released.
#[tokio::test]
async fn idle_and_blocked_connections_expire() {
    let limits = Limits {
        lifetime: Duration::from_millis(150),
        ..Limits::default()
    };
    for mode in [Mode::Accept, Mode::BlockFinalReply] {
        let catcher = Catcher::start(mode, limits).await.unwrap();
        let mut client = Client::connect(&catcher).await;
        if mode == Mode::BlockFinalReply {
            client.envelope().await;
            client.data().await;
            observed(&catcher, |c| c.blocked == 1).await;
        }
        assert!(
            client.reply().await.is_none(),
            "connection lifetime unbounded"
        );
        observed(&catcher, |c| c.active == 0 && c.blocked == 0).await;
        assert_eq!(
            catcher.counts().stored,
            usize::from(mode == Mode::BlockFinalReply)
        );
        catcher.shutdown().await;
    }
}

// Catches unbounded allocation, incomplete DATA acceptance and lenient line endings.
#[tokio::test]
async fn malformed_oversized_or_incomplete_input_is_never_stored() {
    for case in 0..5 {
        let limits = Limits {
            line_bytes: 128,
            body_bytes: 64,
            ..Limits::default()
        };
        let catcher = Catcher::start(Mode::Accept, limits).await.unwrap();
        let mut client = Client::connect(&catcher).await;
        match case {
            0 => client.write(&[b'x'; 129]).await,
            1 => client.write(b"EHLO bad\n").await,
            _ => {
                client.envelope().await;
                client.write(b"DATA\r\n").await;
                assert!(client.reply().await == Some(354), "DATA not admitted");
                match case {
                    2 => {
                        client
                            .write(&[&[b'x'; 65][..], b"\r\n.\r\n"].concat())
                            .await
                    }
                    3 => client.write(b"partial\r\n").await,
                    _ => client.write(b"bad\n.\r\n").await,
                }
            }
        }
        if case == 3 {
            client.0.get_mut().shutdown().await.unwrap();
        }
        assert!(
            client.reply().await.is_none(),
            "invalid input remained open"
        );
        assert_eq!(catcher.counts().stored, 0);
        assert_eq!(catcher.counts().complete_data, 0);
        catcher.shutdown().await;
    }
}

// Catches count caps being advisory or DATA being admitted without an envelope.
#[tokio::test]
async fn message_recipient_command_and_connection_budgets_are_enforced() {
    let catcher = Catcher::start(
        Mode::Accept,
        Limits {
            messages: 1,
            ..Limits::default()
        },
    )
    .await
    .unwrap();
    let mut client = Client::connect(&catcher).await;
    client.write(b"DATA\r\n").await;
    assert!(client.reply().await == Some(503), "empty envelope admitted");
    client.envelope().await;
    client.data().await;
    assert!(client.reply().await == Some(250));
    client.envelope().await;
    client.write(b"DATA\r\n").await;
    assert!(client.reply().await == Some(452), "message budget ignored");
    assert_eq!(catcher.counts().stored, 1);
    catcher.shutdown().await;

    let catcher = Catcher::start(
        Mode::Accept,
        Limits {
            recipients: 1,
            ..Limits::default()
        },
    )
    .await
    .unwrap();
    let mut client = Client::connect(&catcher).await;
    for command in [
        &b"HELO fixture\r\n"[..],
        b"MAIL FROM:<a@example.test>\r\n",
        b"RCPT TO:<b@example.test>\r\n",
    ] {
        client.write(command).await;
        assert!(client.reply().await == Some(250));
    }
    client.write(b"RCPT TO:<c@example.test>\r\n").await;
    assert!(
        client.reply().await == Some(452),
        "recipient budget ignored"
    );
    client.write(b"RSET\r\n").await;
    assert!(client.reply().await == Some(250));
    client.write(b"DATA\r\n").await;
    assert!(client.reply().await == Some(503), "RSET kept envelope");
    catcher.shutdown().await;

    let catcher = Catcher::start(
        Mode::Accept,
        Limits {
            commands: 1,
            connections: 1,
            ..Limits::default()
        },
    )
    .await
    .unwrap();
    let mut client = Client::connect(&catcher).await;
    client.write(b"NOOP\r\n").await;
    assert!(client.reply().await == Some(250));
    assert!(client.reply().await.is_none(), "command budget ignored");
    observed(&catcher, |c| c.active == 0).await;
    assert!(
        TcpStream::connect(catcher.address()).await.is_err(),
        "listener budget ignored"
    );
    catcher.shutdown().await;
}

// Catches detached listener/connection tasks surviving fixture ownership.
#[tokio::test]
async fn dropping_owner_cancels_listener_and_live_connections() {
    let catcher = Catcher::start(Mode::BlockFinalReply, Limits::default())
        .await
        .unwrap();
    let address = catcher.address();
    let mut client = Client::connect(&catcher).await;
    client.envelope().await;
    client.data().await;
    observed(&catcher, |c| c.blocked == 1).await;
    drop(catcher);
    assert!(
        client.reply().await.is_none(),
        "owned connection survived drop"
    );
    assert!(
        TcpStream::connect(address).await.is_err(),
        "owned listener survived drop"
    );
    let mut catcher = Catcher::start(Mode::BlockFinalReply, Limits::default())
        .await
        .unwrap();
    let address = catcher.address();
    let mut client = Client::connect(&catcher).await;
    client.envelope().await;
    client.data().await;
    observed(&catcher, |c| c.blocked == 1).await;
    catcher.close().await;
    assert!(
        catcher.counts().active == 0 && catcher.counts().blocked == 0,
        "explicit cleanup left child tasks"
    );
    assert!(
        !captured(&catcher, 0),
        "explicit cleanup retained private DATA"
    );
    assert!(
        client.reply().await.is_none(),
        "explicit cleanup left owned connection"
    );
    assert!(
        TcpStream::connect(address).await.is_err(),
        "explicit cleanup left listener"
    );
}

// Catches racing DATA commands reserving more memory than the global message cap.
#[tokio::test]
async fn concurrent_data_reserves_budget_before_reading_body() {
    let catcher = Catcher::start(
        Mode::Accept,
        Limits {
            messages: 1,
            ..Limits::default()
        },
    )
    .await
    .unwrap();
    let mut first = Client::connect(&catcher).await;
    let mut second = Client::connect(&catcher).await;
    first.envelope().await;
    second.envelope().await;
    first.write(b"DATA\r\n").await;
    assert!(first.reply().await == Some(354));
    first.write(b"Subject: fixture\r\n\r\nfirst\r\n").await;
    assert_eq!(catcher.counts().stored, 0);
    second.write(b"DATA\r\n").await;
    assert!(
        second.reply().await == Some(452),
        "concurrent budget exceeded"
    );
    first.write(b"..dot\r\n...two\r\n.\r\n").await;
    assert!(first.reply().await == Some(250));
    assert_eq!(catcher.counts().data_attempts, 2);
    assert_eq!(catcher.counts().stored, 1);
    assert!(captured(&catcher, 0), "partial DATA was counted or changed");
    catcher.shutdown().await;
}

#[tokio::test]
async fn zero_or_excessive_limits_are_rejected_before_binding() {
    for limits in [
        Limits {
            messages: 0,
            ..Limits::default()
        },
        Limits {
            body_bytes: usize::MAX,
            ..Limits::default()
        },
        Limits {
            lifetime: Duration::ZERO,
            ..Limits::default()
        },
    ] {
        let result = Catcher::start(Mode::Accept, limits).await;
        assert!(
            matches!(result, Err(e) if e.kind() == io::ErrorKind::InvalidInput),
            "invalid limits admitted"
        );
    }
}
