use super::*;

#[derive(Clone, Copy, Debug)]
struct Packet {
    connection: u16,
    report: u8,
    data: [u8; 8],
    size: usize,
}

fn record(
    packets: &mut Vec<Packet>,
    blocked: u16,
    connection: u16,
    report: u8,
    data: &[u8],
) -> bool {
    if connection == blocked {
        return false;
    }

    let mut copy = [0u8; 8];
    copy[..data.len()].copy_from_slice(data);
    packets.push(Packet {
        connection,
        report,
        data: copy,
        size: data.len(),
    });
    true
}

fn online(router: &mut Router, packets: &mut Vec<Packet>, connection: u16, slot: usize) {
    assert!(router.connect(connection));
    assert!(router.authorize(connection, slot));
    router.subscribe(connection, 7);
    router.flush(|conn, report, data| record(packets, u16::MAX, conn, report, data));
}

fn assert_zero(packets: &[Packet]) {
    for packet in packets {
        assert!(packet.data[..packet.size].iter().all(|byte| *byte == 0));
        assert_eq!(packet.size, report_size(packet.report));
    }
}

#[test]
fn simultaneous_links_and_target_switching_release_both_sides() {
    let mut router = Router::new(3, 0);
    let mut packets = Vec::new();
    online(&mut router, &mut packets, 10, 0);
    online(&mut router, &mut packets, 20, 1);
    online(&mut router, &mut packets, 30, 2);
    assert!(router.ready());
    packets.clear();

    let key = [1, 0, 4, 0, 0, 0, 0, 0];
    let mouse = [1, 10, 0, 0, 0, 0, 0];
    let consumer = [0xcd, 0];
    assert!(router.send(1, &key, |c, r, d| record(&mut packets, u16::MAX, c, r, d)));
    assert!(router.send(2, &mouse, |c, r, d| record(&mut packets, u16::MAX, c, r, d)));
    assert!(router.send(3, &consumer, |c, r, d| record(
        &mut packets,
        u16::MAX,
        c,
        r,
        d
    )));
    assert!(packets.iter().all(|packet| packet.connection == 10));
    assert!(router.authorize(10, 0));
    assert_eq!(router.peer(10).unwrap().releases, 0);

    let before = packets.len();
    assert!(router.select_and_flush(1, |c, r, d| record(&mut packets, u16::MAX, c, r, d)));
    assert_eq!(packets.len(), before + 6);
    assert_zero(&packets[before..]);
    assert!(
        router
            .peers()
            .iter()
            .filter(|peer| peer.connected)
            .all(|peer| peer.encrypted)
    );
    assert_eq!(router.connected_count(), 3);

    assert!(router.send(1, &key, |c, r, d| record(&mut packets, u16::MAX, c, r, d)));
    assert_eq!(packets.last().unwrap().connection, 20);
}

#[test]
fn congestion_defers_old_release_without_blocking_new_target() {
    let mut router = Router::new(3, 0);
    let mut packets = Vec::new();
    online(&mut router, &mut packets, 10, 0);
    online(&mut router, &mut packets, 20, 1);
    online(&mut router, &mut packets, 30, 2);
    assert!(router.select_and_flush(1, |c, r, d| record(&mut packets, u16::MAX, c, r, d)));

    let blocked = 20;
    assert!(router.select_and_flush(2, |c, r, d| record(&mut packets, blocked, c, r, d)));
    assert_eq!(router.peer(20).unwrap().releases, 7);
    assert!(router.ready());

    let key = [1, 0, 4, 0, 0, 0, 0, 0];
    assert!(router.send(1, &key, |c, r, d| record(&mut packets, blocked, c, r, d)));
    assert_eq!(packets.last().unwrap().connection, 30);

    let before = packets.len();
    router.flush(|c, r, d| record(&mut packets, u16::MAX, c, r, d));
    assert_eq!(packets.len(), before + 3);
    assert_zero(&packets[before..]);
    assert_eq!(router.peer(20).unwrap().releases, 0);
}

#[test]
fn offline_target_keeps_other_links_alive() {
    let mut router = Router::new(3, 0);
    let mut packets = Vec::new();
    online(&mut router, &mut packets, 10, 0);
    online(&mut router, &mut packets, 20, 1);
    online(&mut router, &mut packets, 30, 2);
    router.select_and_flush(2, |c, r, d| record(&mut packets, u16::MAX, c, r, d));

    router.disconnect(30);
    assert!(!router.ready());
    let key = [1, 0, 4, 0, 0, 0, 0, 0];
    assert!(!router.send(1, &key, |_, _, _| true));
    assert!(router.peer(10).is_some());
    assert!(router.peer(20).is_some());

    online(&mut router, &mut packets, 31, 2);
    assert!(router.ready());
    assert!(!router.connect(31));
    assert!(router.connect(99));
    assert!(!router.authorize(99, 2));
    router.disconnect(99);
    assert!(router.peer(31).is_some());
    router.disconnect(99);
    assert!(router.peer(31).is_some());
}

#[test]
fn per_peer_cccd_suspend_and_encryption_gates() {
    let mut router = Router::new(3, 0);
    let mut packets = Vec::new();
    online(&mut router, &mut packets, 10, 0);
    online(&mut router, &mut packets, 20, 1);
    online(&mut router, &mut packets, 30, 2);
    router.select_and_flush(2, |c, r, d| record(&mut packets, u16::MAX, c, r, d));
    router.subscribe(30, SUB_KEYBOARD);

    let key = [1, 0, 4, 0, 0, 0, 0, 0];
    let mouse = [1, 10, 0, 0, 0, 0, 0];
    assert!(!router.send(2, &mouse, |_, _, _| true));
    assert!(router.send(1, &key, |_, _, _| true));

    router.peer_mut(30).unwrap().suspended = true;
    assert!(!router.ready());
    router.select_and_flush(0, |c, r, d| record(&mut packets, u16::MAX, c, r, d));
    assert!(router.ready());
    assert!(router.peer(30).unwrap().suspended);
    assert!(!router.send(0, &key, |_, _, _| true));
    assert!(!router.send(1, &key[..7], |_, _, _| true));

    router.peer_mut(10).unwrap().encrypted = false;
    assert!(!router.send(1, &key, |_, _, _| true));
}

#[test]
fn six_slot_capacity_keeps_all_links_independent() {
    let mut router = Router::new(6, 0);
    let mut packets = Vec::new();
    for slot in 0..6 {
        online(&mut router, &mut packets, 100 + slot as u16, slot);
    }

    assert!(!router.connect(200));
    let key = [1, 0, 4, 0, 0, 0, 0, 0];
    for slot in 0..6 {
        router.select_and_flush(slot, |c, r, d| record(&mut packets, u16::MAX, c, r, d));
        assert!(router.send(1, &key, |c, r, d| record(&mut packets, u16::MAX, c, r, d)));
        assert_eq!(packets.last().unwrap().connection, 100 + slot as u16);

        for other in 0..6 {
            assert!(router.peer(100 + other).is_some());
        }
    }
}

#[test]
fn failed_delivery_preserves_state_and_release_debt() {
    let mut router = Router::new(3, 0);
    let mut packets = Vec::new();
    online(&mut router, &mut packets, 10, 0);

    let key = [0, 0, 4, 0, 0, 0, 0, 0];
    assert!(!router.send(1, &key, |_, _, _| false));
    assert_eq!(router.peer(10).unwrap().values[0], [0; 8]);

    router.release_target();
    router.flush(|_, _, _| false);
    assert_eq!(router.peer(10).unwrap().releases, 7);
    assert!(!router.ready());
    assert!(!router.send(1, &key, |_, _, _| panic!("release must precede input")));

    router.flush(|c, r, d| record(&mut packets, u16::MAX, c, r, d));
    assert!(router.ready());
    assert!(router.send(1, &key, |_, _, _| true));
    assert_eq!(router.peer(10).unwrap().values[0], key);
}

#[test]
fn offline_selection_does_not_fall_back_and_back_switch_keeps_links() {
    let mut router = Router::new(3, 0);
    let mut packets = Vec::new();
    online(&mut router, &mut packets, 10, 0);
    online(&mut router, &mut packets, 20, 1);
    assert!(router.select_and_flush(2, |c, r, d| record(&mut packets, u16::MAX, c, r, d)));
    assert!(!router.ready());
    assert!(!router.send(1, &[0; 8], |_, _, _| panic!(
        "offline target must not notify"
    )));
    assert_eq!(router.connected_count(), 2);
    assert!(router.select_and_flush(0, |c, r, d| record(&mut packets, u16::MAX, c, r, d)));
    assert!(router.ready());
    assert_eq!(router.peer_for_slot(1).unwrap().connection, 20);
}

#[test]
fn partial_release_failure_retries_only_the_failed_report() {
    let mut router = Router::new(3, 0);
    let mut packets = Vec::new();
    online(&mut router, &mut packets, 10, 0);
    router.release_target();
    router.flush(|_, report, _| report != 2);
    assert_eq!(router.peer(10).unwrap().releases, SUB_MOUSE);
    assert!(!router.ready());
    packets.clear();
    router.flush(|c, r, d| record(&mut packets, u16::MAX, c, r, d));
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].report, 2);
    assert!(router.ready());
}

#[test]
fn forgetting_one_connection_can_gate_it_without_affecting_the_other() {
    let mut router = Router::new(3, 0);
    let mut packets = Vec::new();
    online(&mut router, &mut packets, 10, 0);
    online(&mut router, &mut packets, 20, 1);
    router.peer_mut(10).unwrap().encrypted = false;
    assert!(!router.ready());
    router.select_and_flush(1, |c, r, d| record(&mut packets, u16::MAX, c, r, d));
    assert!(router.ready());
    router.disconnect(10);
    assert!(router.ready());
    assert_eq!(router.peer_for_slot(1).unwrap().connection, 20);
}
