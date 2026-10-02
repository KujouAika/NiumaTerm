use hyper_util::client::proxy::matcher::{Intercept, Matcher};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::http::Uri;

use crate::http_connect;

fn intercept(proxy: &str) -> Intercept {
    Matcher::builder()
        .all(proxy)
        .build()
        .intercept(&Uri::from_static("https://relay.example:443"))
        .unwrap()
}

/// Run a one-shot HTTP proxy that answers the first request with `reply`
/// and returns that request.
async fn fake_proxy(reply: &'static [u8]) -> (u16, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();

        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await.unwrap());
        }

        socket.write_all(reply).await.unwrap();

        String::from_utf8(request).unwrap()
    });

    (port, task)
}

#[tokio::test]
async fn a_tunnel_keeps_the_bytes_that_follow_the_proxy_reply() {
    // The proxy's reply and the first tunneled bytes arrive in one segment;
    // reading past the blank line would lose the start of the TLS handshake.
    let (port, proxy) = fake_proxy(b"HTTP/1.1 200 Connection established\r\n\r\nTLS").await;

    let mut tunnel = http_connect(
        &intercept(&format!("http://user:secret@127.0.0.1:{port}")),
        "relay.example",
        443,
    )
    .await
    .unwrap();

    let mut first = [0; 3];

    tunnel.read_exact(&mut first).await.unwrap();

    assert_eq!(&first, b"TLS");

    let request = proxy.await.unwrap();

    assert!(request.starts_with("CONNECT relay.example:443 HTTP/1.1\r\n"));
    assert!(request.contains("Proxy-Authorization: Basic dXNlcjpzZWNyZXQ=\r\n"));
}

#[tokio::test]
async fn a_refused_tunnel_is_an_error() {
    let (port, _proxy) = fake_proxy(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n").await;

    let error = http_connect(
        &intercept(&format!("http://127.0.0.1:{port}")),
        "relay.example",
        443,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("407"), "{error}");
}
