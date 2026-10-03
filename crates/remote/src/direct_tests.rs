use std::net::SocketAddr;

use nmt_remote_core::direct::{NatKind, Side};
use tokio::net::UdpSocket;

use crate::direct::{DirectSocket, gather, prepare};
use crate::link::{recv_binary, send_binary};

/// A STUN server on the loopback that reports each request's source port
/// plus `skew`. Next to a server with no skew, a nonzero skew gives the
/// socket that asks both two mapped ports, as a symmetric NAT does.
async fn stun_server(skew: u16) -> String {
    let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = socket.local_addr().unwrap();

    tokio::spawn(async move {
        let mut buf = [0u8; 512];

        while let Ok((len, from)) = socket.recv_from(&mut buf).await {
            if len < 20 {
                continue;
            }

            let SocketAddr::V4(from) = from else {
                continue;
            };

            let mut response = buf[..20].to_vec();

            response[0..2].copy_from_slice(&0x0101u16.to_be_bytes());
            response[2..4].copy_from_slice(&12u16.to_be_bytes());
            response.extend_from_slice(&[0x00, 0x20, 0x00, 0x08, 0x00, 0x01]);
            response.extend_from_slice(&((from.port() + skew) ^ 0x2112).to_be_bytes());

            for (byte, mask) in from.ip().octets().iter().zip([0x21, 0x12, 0xA4, 0x42]) {
                response.push(byte ^ mask);
            }

            let _ = socket.send_to(&response, from).await;
        }
    });

    addr.to_string()
}

/// Gather both sides, connect them over the direct path, and return the
/// client's and the host's socket.
async fn connect(client_stun: &[String], host_stun: &[String]) -> (DirectSocket, DirectSocket) {
    let client = gather(client_stun).await.unwrap();
    let host = gather(host_stun).await.unwrap();

    let client_offer = client.offer();
    let host_offer = host.offer();

    let host_side = tokio::spawn(async move {
        prepare(host, &client_offer, Side::Host)
            .unwrap()
            .connect()
            .await
            .unwrap()
    });

    let (mut client_ws, _) = prepare(client, &host_offer, Side::Client)
        .unwrap()
        .connect()
        .await
        .unwrap();

    // The host sees the stream only once the client writes on it.
    send_binary(&mut client_ws, b"ping".to_vec()).await.unwrap();

    let (host_ws, _) = host_side.await.unwrap();

    (client_ws, host_ws)
}

async fn exchange(mut client: DirectSocket, mut host: DirectSocket) {
    assert_eq!(recv_binary(&mut host).await.unwrap(), b"ping");

    send_binary(&mut host, b"pong".to_vec()).await.unwrap();

    assert_eq!(recv_binary(&mut client).await.unwrap(), b"pong");
}

#[tokio::test]
async fn gathering_classifies_one_port_everywhere_as_cone() {
    let servers = vec![stun_server(0).await, stun_server(0).await];

    let offer = gather(&servers).await.unwrap().offer();

    assert_eq!(offer.nat, NatKind::Cone);
    assert_eq!(offer.addrs.len(), 1);
}

#[tokio::test]
async fn gathering_moves_to_the_next_batch_when_one_has_too_few_answers() {
    // Eight addresses with nobody listening, as a network that blocks every
    // server of the first batch leaves them.
    let mut servers = Vec::new();

    for _ in 0..8 {
        let socket = UdpSocket::bind("127.0.0.1:0").await.unwrap();

        servers.push(socket.local_addr().unwrap().to_string());
    }

    servers.push(stun_server(0).await);
    servers.push(stun_server(0).await);

    assert_eq!(gather(&servers).await.unwrap().offer().nat, NatKind::Cone);
}

#[tokio::test]
async fn a_client_dials_a_waiting_cone_host() {
    let servers = vec![stun_server(0).await, stun_server(0).await];

    let (client, host) = connect(&servers, &servers).await;

    exchange(client, host).await;
}

#[tokio::test]
async fn a_symmetric_host_dials_a_waiting_cone_client() {
    let exact = stun_server(0).await;

    let client_stun = vec![exact.clone(), stun_server(0).await];
    let host_stun = vec![exact, stun_server(7).await];

    assert_eq!(
        gather(&host_stun).await.unwrap().offer().nat,
        NatKind::Symmetric
    );

    let (client, host) = connect(&client_stun, &host_stun).await;

    exchange(client, host).await;
}
