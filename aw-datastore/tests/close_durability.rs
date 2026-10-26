//! `Datastore::close` racing active writers on the shared worker.
//!
//! aw-tauri shuts down by closing the server's datastore while tray and API
//! clones may still be writing. Every write the worker acknowledged before
//! Close must be durable afterwards, and Close must not be starved by them.
//! Close does not drain in-flight HTTP handlers; this covers worker requests.

use aw_datastore::Datastore;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

const WRITERS: usize = 4;
const ROUNDS: usize = 3;

#[test]
fn close_keeps_every_acknowledged_write_from_racing_writers() {
    for round in 0..ROUNDS {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("close-{round}.db"));
        let path = path.to_str().unwrap().to_owned();
        let ds = Datastore::new(path.clone(), false);
        ds.set_key_value_and_commit("initial", "ready").unwrap();

        let barrier = Arc::new(Barrier::new(WRITERS + 1));
        let closing = Arc::new(AtomicBool::new(false));
        let (writer_done_tx, writer_done_rx) = mpsc::channel();
        let writers: Vec<_> = (0..WRITERS)
            .map(|id| {
                let worker = ds.clone();
                let barrier = barrier.clone();
                let closing = closing.clone();
                let writer_done_tx = writer_done_tx.clone();
                thread::spawn(move || {
                    let key = format!("writer-{id}");
                    // Each writer holds an acknowledged value before Close races it.
                    worker.set_key_value(&key, "0").unwrap();
                    let mut last_ok = 0;
                    barrier.wait();
                    for n in 1..10_000 {
                        match worker.set_key_value(&key, &n.to_string()) {
                            Ok(()) => last_ok = n,
                            Err(_) => {
                                assert!(
                                    closing.load(Ordering::SeqCst),
                                    "write failed before Close"
                                );
                                break;
                            }
                        }
                    }
                    writer_done_tx.send(()).unwrap();
                    (key, last_ok)
                })
            })
            .collect();

        barrier.wait();
        closing.store(true, Ordering::SeqCst);
        let closer = ds.clone();
        let (done_tx, done_rx) = mpsc::channel();
        thread::spawn(move || {
            closer.close();
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("active writers starved Close");

        drop(writer_done_tx);
        let deadline = Instant::now() + Duration::from_secs(5);
        for _ in 0..WRITERS {
            writer_done_rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("writer did not finish after Close");
        }
        let acknowledged: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();
        let reader = Datastore::open_reader(path).unwrap();
        for (key, value) in acknowledged {
            assert_eq!(
                reader.get_key_value(&key).unwrap(),
                value.to_string(),
                "round {round}: acknowledged write to {key} lost by Close"
            );
        }
        reader.close();
        assert!(
            ds.get_buckets().is_err(),
            "closed worker still served a request"
        );
    }
}
