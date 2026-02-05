// src/parser/tests.rs
use super::*;
use crate::broker::Broker;
use state::Format;
use tokio_stream::StreamExt;

#[tokio::test]
async fn test_parser_spawn() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    // Should be able to query immediately
    let response = parser
        .query(Query::Screen {
            format: Format::Plain,
        })
        .await
        .unwrap();

    match response {
        QueryResponse::Screen(screen) => {
            assert_eq!(screen.cols, 80);
            assert_eq!(screen.rows, 24);
        }
        _ => panic!("expected Screen response"),
    }
}

#[tokio::test]
async fn test_parser_query_cursor() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    let response = parser.query(Query::Cursor).await.unwrap();

    match response {
        QueryResponse::Cursor(cursor_resp) => {
            assert_eq!(cursor_resp.cursor.row, 0);
            assert_eq!(cursor_resp.cursor.col, 0);
        }
        _ => panic!("expected Cursor response"),
    }
}

#[tokio::test]
async fn test_parser_processes_input() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    // Send some text through the broker
    broker.publish(bytes::Bytes::from("Hello, World!"));

    // Give the parser time to process
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

    let response = parser
        .query(Query::Screen {
            format: Format::Plain,
        })
        .await
        .unwrap();

    match response {
        QueryResponse::Screen(screen) => {
            assert!(!screen.lines.is_empty());
            if let Some(state::FormattedLine::Plain(text)) = screen.lines.first() {
                assert!(text.contains("Hello"));
            }
        }
        _ => panic!("expected Screen response"),
    }
}

#[tokio::test]
async fn test_parser_resize() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    // Resize
    parser.resize(120, 40).await.unwrap();

    // Query screen to verify new size
    let response = parser
        .query(Query::Screen {
            format: Format::Plain,
        })
        .await
        .unwrap();

    match response {
        QueryResponse::Screen(screen) => {
            assert_eq!(screen.cols, 120);
            assert_eq!(screen.rows, 40);
        }
        _ => panic!("expected Screen response"),
    }
}

#[tokio::test]
async fn test_parser_scrollback() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 5, 100); // Small screen for testing

    // Send enough lines to create scrollback
    for i in 0..10 {
        broker.publish(bytes::Bytes::from(format!("Line {}\r\n", i)));
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let response = parser
        .query(Query::Scrollback {
            format: Format::Plain,
            offset: 0,
            limit: 100,
        })
        .await
        .unwrap();

    match response {
        QueryResponse::Scrollback(scrollback) => {
            // Should have some scrollback
            assert!(scrollback.total_lines > 0);
        }
        _ => panic!("expected Scrollback response"),
    }
}

#[tokio::test]
async fn test_parser_event_stream() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    let mut events = parser.subscribe();

    // Send text
    broker.publish(bytes::Bytes::from("Test"));

    // Should receive events
    let event = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        events.next(),
    )
    .await;

    assert!(event.is_ok(), "should receive an event");
}
