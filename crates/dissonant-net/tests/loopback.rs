//! Real sockets, real encryption, on this machine.

use dissonant_net::{new_secret, Invite, Net, NetEvent, Reach, HOST};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

fn next(rx: &Receiver<NetEvent>) -> NetEvent {
    rx.recv_timeout(Duration::from_secs(20)).expect("nothing arrived")
}

fn pair() -> (Net, Receiver<NetEvent>, Net, Receiver<NetEvent>, u64) {
    let (host_tx, host_rx) = channel();
    let (guest_tx, guest_rx) = channel();
    let (host, addr) = Net::host(Reach::Direct, host_tx).unwrap();
    let guest = Net::join(Reach::Direct, addr, guest_tx).unwrap();
    assert_eq!(next(&guest_rx), NetEvent::Connected(HOST));
    // A QUIC stream only exists at the far end once something is written to it.
    guest.send(HOST, b"hello");
    let NetEvent::Connected(conn) = next(&host_rx) else { panic!("the host saw no connection") };
    assert_eq!(next(&host_rx), NetEvent::Message(conn, b"hello".to_vec()));
    (host, host_rx, guest, guest_rx, conn)
}

#[test]
fn messages_arrive_in_order_both_ways() {
    let (host, host_rx, guest, guest_rx, conn) = pair();
    for i in 0..200u32 {
        guest.send(HOST, &i.to_be_bytes());
        host.send(conn, &(i + 1000).to_be_bytes());
    }
    for i in 0..200u32 {
        assert_eq!(next(&host_rx), NetEvent::Message(conn, i.to_be_bytes().to_vec()));
        assert_eq!(next(&guest_rx), NetEvent::Message(HOST, (i + 1000).to_be_bytes().to_vec()));
    }
    host.shutdown();
    guest.shutdown();
}

#[test]
fn a_large_project_gets_through() {
    let (host, _host_rx, guest, guest_rx, conn) = pair();
    let big = vec![7u8; 8 * 1024 * 1024];
    host.send(conn, &big);
    assert_eq!(next(&guest_rx), NetEvent::Message(HOST, big));
    host.shutdown();
    guest.shutdown();
}

#[test]
fn datagrams_get_through() {
    let (host, host_rx, guest, _guest_rx, conn) = pair();
    // Unreliable by design, so one lost packet must not fail the test.
    for _ in 0..20 {
        guest.datagram(HOST, b"cursor");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(next(&host_rx), NetEvent::Datagram(conn, b"cursor".to_vec()));
    host.shutdown();
    guest.shutdown();
}

#[test]
fn a_closed_connection_delivers_what_was_queued_first() {
    let (host, _host_rx, guest, guest_rx, conn) = pair();
    host.send(conn, b"session over");
    host.close(conn);
    assert_eq!(next(&guest_rx), NetEvent::Message(HOST, b"session over".to_vec()));
    host.shutdown();
    guest.shutdown();
}

#[test]
fn an_invite_survives_being_pasted() {
    let (tx, _rx) = channel();
    let (host, addr) = Net::host(Reach::Direct, tx).unwrap();
    let invite = Invite { addr, secret: new_secret() };
    let code = format!("  {}\n", invite.to_code());
    assert_eq!(Invite::parse(&code).unwrap(), invite);
    assert!(Invite::parse("hello there").is_err());
    host.shutdown();
}

/// Takes about two minutes: a guest only learns a crashed host is gone by hearing nothing. Run it
/// with `cargo test -p dissonant-net --test loopback -- --ignored --nocapture`.
#[test]
#[ignore]
fn a_guest_gives_up_on_a_host_that_vanished() {
    let (host, _host_rx, guest, guest_rx, _conn) = pair();
    let started = std::time::Instant::now();
    // No goodbye: this is what a crash looks like from the far end.
    drop(host);
    let mut lost = None;
    loop {
        match guest_rx.recv_timeout(Duration::from_secs(180)).expect("the guest never gave up") {
            NetEvent::Disconnected(HOST) => lost = Some(started.elapsed()),
            NetEvent::GaveUp(why) => {
                println!("lost after {lost:?}, gave up after {:?}: {why}", started.elapsed());
                assert!(lost.is_some(), "it noticed the loss first");
                break;
            }
            _ => {}
        }
    }
    guest.shutdown();
}
