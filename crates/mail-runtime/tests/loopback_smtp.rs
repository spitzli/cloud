//! Plain transport is an explicit loopback adapter using the same post-DATA classifier.
use std::time::Duration;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};
use webdock_mail_runtime::smtp::submit_loopback;
async fn attempt(mode: u8) -> Result<(), webdock_mail_runtime::smtp::SmtpFailure> {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let (read, mut write) = socket.into_split();
        let mut lines = BufReader::new(read).lines();
        write.write_all(b"220 catcher\r\n").await.unwrap();
        let mut data = false;
        while let Ok(Some(line)) = lines.next_line().await {
            let reply = if data {
                if line != "." {
                    continue;
                }
                if mode == 3 {
                    return;
                }
                if mode == 2 {
                    b"450 rejected\r\n".as_slice()
                } else {
                    b"250 accepted\r\n".as_slice()
                }
            } else if line.starts_with("EHLO") {
                b"250 catcher\r\n".as_slice()
            } else if line == "DATA" {
                if mode == 1 {
                    b"450 rejected\r\n".as_slice()
                } else {
                    data = true;
                    b"354 continue\r\n".as_slice()
                }
            } else {
                b"250 ok\r\n".as_slice()
            };
            if write.write_all(reply).await.is_err() {
                return;
            }
        }
    });
    let result = submit_loopback(
        "127.0.0.1",
        port,
        "from@example.test",
        &["to@example.test".into()],
        b"From: from@example.test\r\nTo: to@example.test\r\nSubject: owned\r\n\r\nbody\r\n",
        Duration::from_secs(2),
    )
    .await;
    server.await.unwrap();
    result
}
#[tokio::test]
async fn local_adapter_classifies_accept_reject_and_lost_final_without_retry() {
    assert!(attempt(0).await.is_ok());
    for mode in [1, 2] {
        let e = attempt(mode).await.unwrap_err();
        assert!(!e.uncertain);
        assert_eq!(e.response_code, Some(450));
    }
    assert!(attempt(3).await.unwrap_err().uncertain);
    assert!(
        submit_loopback(
            "remote.invalid",
            25,
            "from@example.test",
            &["to@example.test".into()],
            b"body",
            Duration::from_secs(1)
        )
        .await
        .is_err()
    );
}
