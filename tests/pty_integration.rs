//! Integration tests for PTY + Broker round-trip functionality.
//!
//! These tests verify that the PTY module and broker work together correctly:
//! - PTY output is received by broker subscribers
//! - Write to PTY via channel, output broadcasts to subscribers
//! - Multiple subscribers all receive PTY output

use bytes::Bytes;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use wsh::{broker::Broker, pty::Pty};

/// Helper to read from PTY with a timeout to avoid blocking forever.
/// The reader thread checks the stop flag periodically.
/// Returns the bytes read.
fn read_pty_and_publish(
    mut reader: Box<dyn Read + Send>,
    broker: Broker,
    stop_flag: Arc<AtomicBool>,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut collected = Vec::new();

        // Set read to be non-blocking-ish by using small reads with checks
        while !stop_flag.load(Ordering::Relaxed) {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    let data = Bytes::copy_from_slice(&buf[..n]);
                    collected.extend_from_slice(&buf[..n]);
                    broker.publish(data);
                }
                Err(e) => {
                    // EIO is expected when PTY slave is closed
                    if e.raw_os_error() != Some(5) {
                        eprintln!("PTY read error: {:?}", e);
                    }
                    break;
                }
            }
        }
        collected
    })
}

#[test]
fn test_pty_output_broadcasts_to_subscribers() {
    // Create PTY
    let pty = Pty::spawn(24, 80).expect("Failed to spawn PTY");
    let mut writer = pty.take_writer().expect("Failed to get writer");
    let reader = pty.take_reader().expect("Failed to get reader");

    // Create broker and subscribe
    let broker = Broker::new();
    let mut rx = broker.subscribe();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let _reader_handle = read_pty_and_publish(reader, broker.clone(), stop_flag.clone());

    // Give the reader thread time to start
    thread::sleep(Duration::from_millis(100));

    // Write a command to PTY that will produce output
    let marker = "PTY_TEST_BROADCAST_12345";
    let cmd = format!("echo {}\n", marker);
    writer.write_all(cmd.as_bytes()).expect("Write failed");
    writer.flush().expect("Flush failed");

    // Collect messages from broker with timeout
    let (tx, result_rx) = mpsc::channel();
    let marker_clone = marker.to_string();
    thread::spawn(move || {
        let mut collected = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(data) => {
                    collected.extend_from_slice(&data);
                    let output_str = String::from_utf8_lossy(&collected);
                    if output_str.contains(&marker_clone) {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(collected);
    });

    let collected = result_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("Timed out waiting for broker messages");

    // Verify we received something and it contains our marker
    let output_str = String::from_utf8_lossy(&collected);
    assert!(
        output_str.contains(marker),
        "Expected output to contain '{}', but got: {}",
        marker,
        output_str
    );

    // Signal reader to stop and clean up
    stop_flag.store(true, Ordering::Relaxed);
    // Send exit to shell to close PTY gracefully
    let _ = writer.write_all(b"exit\n");
    let _ = writer.flush();
}

#[test]
fn test_full_pty_roundtrip_with_broker() {
    // Create PTY
    let pty = Pty::spawn(24, 80).expect("Failed to spawn PTY");
    let mut pty_writer = pty.take_writer().expect("Failed to get writer");
    let reader = pty.take_reader().expect("Failed to get reader");

    // Create broker and subscribe
    let broker = Broker::new();
    let mut rx = broker.subscribe();

    // Channel for input -> PTY
    let (input_tx, input_rx) = mpsc::channel::<Bytes>();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let _reader_handle = read_pty_and_publish(reader, broker.clone(), stop_flag.clone());

    // Spawn thread to receive from channel and write to PTY
    let writer_handle = thread::spawn(move || {
        while let Ok(data) = input_rx.recv_timeout(Duration::from_secs(3)) {
            if pty_writer.write_all(&data).is_err() {
                break;
            }
            let _ = pty_writer.flush();
        }
    });

    // Give threads time to start
    thread::sleep(Duration::from_millis(100));

    // Send input via the channel (simulating API/WebSocket input)
    let marker = "ROUNDTRIP_TEST_67890";
    let cmd = format!("echo {}\n", marker);
    input_tx.send(Bytes::from(cmd)).expect("Failed to send input");

    // Collect messages from broker with timeout
    let (tx, result_rx) = mpsc::channel();
    let marker_clone = marker.to_string();
    thread::spawn(move || {
        let mut collected = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(data) => {
                    collected.extend_from_slice(&data);
                    let output_str = String::from_utf8_lossy(&collected);
                    if output_str.contains(&marker_clone) {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(collected);
    });

    let collected = result_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("Timed out waiting for broker messages");

    // Verify we received the output
    let output_str = String::from_utf8_lossy(&collected);
    assert!(
        output_str.contains(marker),
        "Expected output to contain '{}', but got: {}",
        marker,
        output_str
    );

    // Clean up
    stop_flag.store(true, Ordering::Relaxed);
    // Send exit to close PTY gracefully
    input_tx.send(Bytes::from("exit\n")).ok();
    drop(input_tx);
    let _ = writer_handle.join();
}

#[test]
fn test_multiple_broker_subscribers_receive_pty_output() {
    // Create PTY
    let pty = Pty::spawn(24, 80).expect("Failed to spawn PTY");
    let mut writer = pty.take_writer().expect("Failed to get writer");
    let reader = pty.take_reader().expect("Failed to get reader");

    // Create broker and multiple subscribers
    let broker = Broker::new();
    let rx1 = broker.subscribe();
    let rx2 = broker.subscribe();
    let rx3 = broker.subscribe();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let _reader_handle = read_pty_and_publish(reader, broker.clone(), stop_flag.clone());

    // Give the reader thread time to start
    thread::sleep(Duration::from_millis(100));

    // Write a command to PTY
    let marker = "MULTI_SUB_TEST_11111";
    let cmd = format!("echo {}\n", marker);
    writer.write_all(cmd.as_bytes()).expect("Write failed");
    writer.flush().expect("Flush failed");

    // Collect from all subscribers concurrently
    let collect_from_rx = |mut rx: tokio::sync::broadcast::Receiver<Bytes>,
                           marker: String|
     -> mpsc::Receiver<Vec<u8>> {
        let (tx, result_rx) = mpsc::channel();
        thread::spawn(move || {
            let mut collected = Vec::new();
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while std::time::Instant::now() < deadline {
                match rx.try_recv() {
                    Ok(data) => {
                        collected.extend_from_slice(&data);
                        let output_str = String::from_utf8_lossy(&collected);
                        if output_str.contains(&marker) {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            let _ = tx.send(collected);
        });
        result_rx
    };

    let result1 = collect_from_rx(rx1, marker.to_string());
    let result2 = collect_from_rx(rx2, marker.to_string());
    let result3 = collect_from_rx(rx3, marker.to_string());

    let collected1 = result1
        .recv_timeout(Duration::from_secs(3))
        .expect("Subscriber 1 timed out");
    let collected2 = result2
        .recv_timeout(Duration::from_secs(3))
        .expect("Subscriber 2 timed out");
    let collected3 = result3
        .recv_timeout(Duration::from_secs(3))
        .expect("Subscriber 3 timed out");

    // Verify all subscribers received the marker
    let output1 = String::from_utf8_lossy(&collected1);
    let output2 = String::from_utf8_lossy(&collected2);
    let output3 = String::from_utf8_lossy(&collected3);

    assert!(
        output1.contains(marker),
        "Subscriber 1 should have received '{}', got: {}",
        marker,
        output1
    );
    assert!(
        output2.contains(marker),
        "Subscriber 2 should have received '{}', got: {}",
        marker,
        output2
    );
    assert!(
        output3.contains(marker),
        "Subscriber 3 should have received '{}', got: {}",
        marker,
        output3
    );

    // Clean up
    stop_flag.store(true, Ordering::Relaxed);
    let _ = writer.write_all(b"exit\n");
    let _ = writer.flush();
}

#[test]
fn test_late_subscriber_receives_future_output() {
    // Create PTY
    let pty = Pty::spawn(24, 80).expect("Failed to spawn PTY");
    let mut writer = pty.take_writer().expect("Failed to get writer");
    let reader = pty.take_reader().expect("Failed to get reader");

    // Create broker (no subscriber yet)
    let broker = Broker::new();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let _reader_handle = read_pty_and_publish(reader, broker.clone(), stop_flag.clone());

    // Give the reader thread time to start
    thread::sleep(Duration::from_millis(100));

    // Write first command (no subscriber yet)
    let marker1 = "EARLY_OUTPUT_22222";
    let cmd1 = format!("echo {}\n", marker1);
    writer.write_all(cmd1.as_bytes()).expect("Write failed");
    writer.flush().expect("Flush failed");

    // Wait a bit for the first output to be processed
    thread::sleep(Duration::from_millis(300));

    // Now subscribe (late subscriber)
    let mut rx = broker.subscribe();

    // Write second command (late subscriber should receive this)
    let marker2 = "LATE_OUTPUT_33333";
    let cmd2 = format!("echo {}\n", marker2);
    writer.write_all(cmd2.as_bytes()).expect("Write failed");
    writer.flush().expect("Flush failed");

    // Collect messages from the late subscriber
    let (tx, result_rx) = mpsc::channel();
    let marker2_clone = marker2.to_string();
    thread::spawn(move || {
        let mut collected = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            match rx.try_recv() {
                Ok(data) => {
                    collected.extend_from_slice(&data);
                    let output_str = String::from_utf8_lossy(&collected);
                    if output_str.contains(&marker2_clone) {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(collected);
    });

    let collected = result_rx
        .recv_timeout(Duration::from_secs(3))
        .expect("Late subscriber timed out");

    // Late subscriber should have received the second output
    let output_str = String::from_utf8_lossy(&collected);
    assert!(
        output_str.contains(marker2),
        "Late subscriber should have received '{}', but got: {}",
        marker2,
        output_str
    );

    // Clean up
    stop_flag.store(true, Ordering::Relaxed);
    let _ = writer.write_all(b"exit\n");
    let _ = writer.flush();
}
