//! The native live-update listener against a local Electrum WebSocket server that pushes real
//! notifications, subscribes new scripts on the open connection and refreshes when dropped.
//! Reconnection timing is covered by the listener's unit tests.
use anzen_wallet::watch::{ChainWatcher, Change, ElectrumWatcher, native::Watch};
use serde_json::{Value, json};
use std::{
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tungstenite::Message;

fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn native_listener_reports_blocks_and_addresses_and_refreshes_when_dropped() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let subscribed = Arc::new(Mutex::new(Vec::<String>::new()));
    // Each accepted connection hands the test a way to push frames or drop it.
    let (connections, accepted) = mpsc::channel::<mpsc::Sender<Option<String>>>();
    let seen = subscribed.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let mut socket = tungstenite::accept(stream.unwrap()).unwrap();
            socket
                .get_ref()
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            let (push, pushes) = mpsc::channel::<Option<String>>();
            connections.send(push).unwrap();
            let seen = seen.clone();
            thread::spawn(move || {
                loop {
                    match pushes.try_recv() {
                        Ok(Some(frame)) => socket.send(Message::text(frame)).unwrap(),
                        Ok(None) => return, // Drop the connection.
                        Err(_) => {}
                    }
                    let Ok(Message::Text(text)) = socket.read() else {
                        continue;
                    };
                    let payload: Value = serde_json::from_str(&text).unwrap();
                    let calls = match &payload {
                        Value::Array(calls) => calls.clone(),
                        call => vec![call.clone()],
                    };
                    let replies: Vec<Value> = calls
                        .iter()
                        .map(|call| {
                            if call["method"] == "blockchain.scripthash.subscribe" {
                                seen.lock()
                                    .unwrap()
                                    .push(call["params"][0].as_str().unwrap().into());
                            }
                            json!({"id": call["id"], "result": null})
                        })
                        .collect();
                    socket
                        .send(Message::text(Value::Array(replies).to_string()))
                        .unwrap();
                }
            });
        }
    });

    let hash = |n: u8| format!("{n:064x}");
    let watch = Watch::spawn(
        ChainWatcher::Electrum(ElectrumWatcher::with_servers(vec![url])),
        None,
    )
    .unwrap();
    watch.set_targets(vec![hash(1), hash(2)]);
    let first = accepted.recv_timeout(Duration::from_secs(10)).unwrap();
    until("subscriptions", || subscribed.lock().unwrap().len() == 2);
    // Establishing the subscription refreshes once, covering anything before it.
    let mut changes = Vec::new();
    until("initial refresh", || {
        changes.extend(watch.changes());
        changes.contains(&Change::Subscribed)
    });

    first
        .send(Some(
            json!({"method":"blockchain.headers.subscribe","params":[{"height":2}]}).to_string(),
        ))
        .unwrap();
    first
        .send(Some(
            json!({"method":"blockchain.scripthash.subscribe","params":[hash(2),"x"]}).to_string(),
        ))
        .unwrap();
    let mut changes = Vec::new();
    until("notifications", || {
        changes.extend(watch.changes());
        changes.contains(&Change::Block) && changes.contains(&Change::Address)
    });

    // New scripts are subscribed on the live connection.
    watch.set_targets(vec![hash(1), hash(2), hash(3)]);
    until("new subscription", || subscribed.lock().unwrap().len() == 3);

    // The server drops the connection: refresh at once instead of waiting for the retry.
    first.send(None).unwrap();
    let mut changes = Vec::new();
    until("refresh after disconnect", || {
        changes.extend(watch.changes());
        changes.contains(&Change::Poll)
    });
    drop(watch);
}
