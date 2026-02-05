// tests/parser_integration.rs
use bytes::Bytes;
use wsh::broker::Broker;
use wsh::parser::state::{Format, Query, QueryResponse};
use wsh::parser::Parser;

#[tokio::test]
async fn test_parser_with_ansi_sequences() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    // Send colored text
    broker.publish(Bytes::from("\x1b[31mRed Text\x1b[0m Normal"));

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let response = parser
        .query(Query::Screen {
            format: Format::Styled,
        })
        .await
        .unwrap();

    match response {
        QueryResponse::Screen(screen) => {
            // Should have parsed the content
            assert!(!screen.lines.is_empty(), "screen should have at least one line");

            // Verify we got a Styled line (since we requested Format::Styled)
            let first_line = screen.lines.first().expect("should have first line");
            match first_line {
                wsh::parser::state::FormattedLine::Styled(spans) => {
                    // Should have styled spans
                    assert!(!spans.is_empty(), "styled line should have spans");

                    // Concatenate all span text to verify content
                    let full_text: String = spans.iter().map(|s| s.text.as_str()).collect();
                    assert!(
                        full_text.contains("Red Text"),
                        "should contain 'Red Text', got: {}",
                        full_text
                    );
                    assert!(
                        full_text.contains("Normal"),
                        "should contain 'Normal', got: {}",
                        full_text
                    );

                    // Verify red color is present in one of the spans
                    let has_red = spans.iter().any(|s| {
                        matches!(
                            s.style.fg,
                            Some(wsh::parser::state::Color::Indexed(1)) // Red is index 1
                        )
                    });
                    assert!(has_red, "should have a span with red foreground color");
                }
                wsh::parser::state::FormattedLine::Plain(_) => {
                    panic!("expected Styled variant when requesting Format::Styled")
                }
            }
        }
        _ => panic!("expected Screen response"),
    }
}

#[tokio::test]
async fn test_parser_cursor_movement() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    // Move cursor to row 5, col 10
    broker.publish(Bytes::from("\x1b[5;10H"));

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    let response = parser.query(Query::Cursor).await.unwrap();

    match response {
        QueryResponse::Cursor(cursor) => {
            // Cursor should have moved (0-indexed)
            assert_eq!(cursor.cursor.row, 4);
            assert_eq!(cursor.cursor.col, 9);
        }
        _ => panic!("expected Cursor response"),
    }
}

#[tokio::test]
async fn test_parser_plain_vs_styled() {
    let broker = Broker::new();
    let parser = Parser::spawn(&broker, 80, 24, 1000);

    broker.publish(Bytes::from("\x1b[1mBold\x1b[0m"));

    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

    // Get plain format
    let plain = parser
        .query(Query::Screen {
            format: Format::Plain,
        })
        .await
        .unwrap();

    // Get styled format
    let styled = parser
        .query(Query::Screen {
            format: Format::Styled,
        })
        .await
        .unwrap();

    match (plain, styled) {
        (QueryResponse::Screen(p), QueryResponse::Screen(s)) => {
            // Both should have at least one line
            assert!(!p.lines.is_empty(), "plain should have lines");
            assert!(!s.lines.is_empty(), "styled should have lines");

            // Verify plain format returns Plain variant
            let plain_line = p.lines.first().expect("plain should have first line");
            match plain_line {
                wsh::parser::state::FormattedLine::Plain(text) => {
                    assert!(
                        text.contains("Bold"),
                        "plain text should contain 'Bold', got: {}",
                        text
                    );
                }
                wsh::parser::state::FormattedLine::Styled(_) => {
                    panic!("expected Plain variant when requesting Format::Plain")
                }
            }

            // Verify styled format returns Styled variant with bold
            let styled_line = s.lines.first().expect("styled should have first line");
            match styled_line {
                wsh::parser::state::FormattedLine::Styled(spans) => {
                    assert!(!spans.is_empty(), "styled line should have spans");

                    // Verify bold text is present
                    let full_text: String = spans.iter().map(|s| s.text.as_str()).collect();
                    assert!(
                        full_text.contains("Bold"),
                        "styled text should contain 'Bold', got: {}",
                        full_text
                    );

                    // Verify at least one span has bold style
                    let has_bold = spans.iter().any(|s| s.style.bold);
                    assert!(has_bold, "should have a span with bold style");
                }
                wsh::parser::state::FormattedLine::Plain(_) => {
                    panic!("expected Styled variant when requesting Format::Styled")
                }
            }
        }
        _ => panic!("expected Screen responses"),
    }
}
