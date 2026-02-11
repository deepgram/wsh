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
async fn test_scrollback_includes_all_lines() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 5, 100); // Small screen for testing

    // Send enough lines to create scrollback
    for i in 0..10 {
        broker.publish(bytes::Bytes::from(format!("Line {}\r\n", i)));
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // First check screen to see total_lines
    let screen_response = parser
        .query(Query::Screen {
            format: Format::Plain,
        })
        .await
        .unwrap();

    let (screen_total_lines, screen_first_line_index) = match &screen_response {
        QueryResponse::Screen(screen) => (screen.total_lines, screen.first_line_index),
        _ => panic!("expected Screen response"),
    };

    // Now check scrollback
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
            // Verify: with 10 lines fed and 5 rows visible, scrollback should contain ALL lines
            // (both history and current screen)
            assert!(screen_total_lines >= 10, "should have at least 10 total lines, got {}", screen_total_lines);
            assert!(screen_first_line_index >= 5, "first_line_index should be >= 5, got {}", screen_first_line_index);
            // Scrollback total_lines should match screen total_lines (all lines in buffer)
            assert_eq!(scrollback.total_lines, screen_total_lines, "scrollback total_lines should equal screen total_lines");
            assert!(!scrollback.lines.is_empty(), "scrollback lines should not be empty");
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

#[tokio::test]
async fn test_line_event_includes_total_lines() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    let mut events = parser.subscribe();

    // Send text to trigger a line event
    broker.publish(bytes::Bytes::from("Hello"));

    // Get the line event
    let event = tokio::time::timeout(
        tokio::time::Duration::from_millis(100),
        events.next(),
    )
    .await
    .expect("should receive event")
    .expect("stream should have item");

    match event {
        Event::Line { total_lines, .. } => {
            assert!(total_lines >= 24, "total_lines should be at least screen height");
        }
        _ => panic!("expected Line event, got {:?}", event),
    }
}

#[tokio::test]
async fn test_scrollback_when_in_alternate_screen() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 5, 100); // Small screen for testing

    // Send enough lines to create scrollback
    for i in 0..10 {
        broker.publish(bytes::Bytes::from(format!("Line {}\r\n", i)));
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Verify we have scrollback before switching to alternate screen
    let response = parser
        .query(Query::Scrollback {
            format: Format::Plain,
            offset: 0,
            limit: 100,
        })
        .await
        .unwrap();

    let scrollback_before = match &response {
        QueryResponse::Scrollback(s) => s.total_lines,
        _ => panic!("expected Scrollback response"),
    };
    assert!(scrollback_before > 0, "Should have scrollback before alternate screen");

    // Enter alternate screen mode (DECSET 1049 or smcup)
    broker.publish(bytes::Bytes::from("\x1b[?1049h")); // Enter alternate screen
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Query scrollback while in alternate screen
    let response = parser
        .query(Query::Scrollback {
            format: Format::Plain,
            offset: 0,
            limit: 100,
        })
        .await
        .unwrap();

    let scrollback_in_alternate = match &response {
        QueryResponse::Scrollback(s) => s.total_lines,
        _ => panic!("expected Scrollback response"),
    };

    // In alternate screen mode, scrollback returns the alternate buffer content
    // (just the current screen, since alternate buffer has no history)
    // Alternate screen should have exactly 5 lines (the screen size)
    assert_eq!(scrollback_in_alternate, 5, "Alternate screen should have screen-size lines");

    // Exit alternate screen mode (DECRST 1049 or rmcup)
    broker.publish(bytes::Bytes::from("\x1b[?1049l")); // Exit alternate screen
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Query scrollback after exiting alternate screen
    let response = parser
        .query(Query::Scrollback {
            format: Format::Plain,
            offset: 0,
            limit: 100,
        })
        .await
        .unwrap();

    let scrollback_after = match &response {
        QueryResponse::Scrollback(s) => s.total_lines,
        _ => panic!("expected Scrollback response"),
    };

    // Scrollback should be preserved after exiting alternate screen
    assert_eq!(scrollback_after, scrollback_before, "Scrollback should be preserved after exiting alternate screen");
}

#[tokio::test]
async fn test_alternate_active_in_screen_response() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    // Initially not in alternate screen
    let response = parser
        .query(Query::Screen {
            format: Format::Plain,
        })
        .await
        .unwrap();

    match &response {
        QueryResponse::Screen(screen) => {
            assert!(!screen.alternate_active, "should start in primary screen");
        }
        _ => panic!("expected Screen response"),
    }

    // Enter alternate screen mode
    broker.publish(bytes::Bytes::from("\x1b[?1049h"));
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let response = parser
        .query(Query::Screen {
            format: Format::Plain,
        })
        .await
        .unwrap();

    match &response {
        QueryResponse::Screen(screen) => {
            assert!(screen.alternate_active, "should be in alternate screen after DECSET 1049");
        }
        _ => panic!("expected Screen response"),
    }

    // Exit alternate screen mode
    broker.publish(bytes::Bytes::from("\x1b[?1049l"));
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let response = parser
        .query(Query::Screen {
            format: Format::Plain,
        })
        .await
        .unwrap();

    match &response {
        QueryResponse::Screen(screen) => {
            assert!(!screen.alternate_active, "should be back in primary screen after DECRST 1049");
        }
        _ => panic!("expected Screen response"),
    }
}

#[tokio::test]
async fn test_alternate_screen_emits_mode_event() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    let mut events = parser.subscribe();

    // Enter alternate screen
    broker.publish(bytes::Bytes::from("\x1b[?1049h"));

    // Collect events until we find a Mode event
    let mode_event = tokio::time::timeout(tokio::time::Duration::from_millis(200), async {
        loop {
            if let Some(event) = events.next().await {
                if let Event::Mode { alternate_active, .. } = event {
                    return alternate_active;
                }
            }
        }
    })
    .await
    .expect("should receive Mode event");

    assert!(mode_event, "Mode event should indicate alternate_active = true");

    // Exit alternate screen
    broker.publish(bytes::Bytes::from("\x1b[?1049l"));

    let mode_event = tokio::time::timeout(tokio::time::Duration::from_millis(200), async {
        loop {
            if let Some(event) = events.next().await {
                if let Event::Mode { alternate_active, .. } = event {
                    return alternate_active;
                }
            }
        }
    })
    .await
    .expect("should receive Mode event on exit");

    assert!(!mode_event, "Mode event should indicate alternate_active = false");
}

#[tokio::test]
async fn test_screen_response_includes_line_indices() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 5, 100); // Small screen

    // Send enough lines to create scrollback
    for i in 0..10 {
        broker.publish(bytes::Bytes::from(format!("Line {}\r\n", i)));
    }
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let response = parser
        .query(Query::Screen { format: Format::Plain })
        .await
        .unwrap();

    match response {
        QueryResponse::Screen(screen) => {
            // With 10 lines and 5-row screen, first_line_index should be 5
            // (lines 0-4 in scrollback, lines 5-9 visible)
            assert!(screen.first_line_index > 0, "should have scrollback");
            assert_eq!(screen.total_lines, screen.first_line_index + screen.lines.len());
        }
        _ => panic!("expected Screen response"),
    }
}
