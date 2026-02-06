use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, oneshot};

use super::events::{Event, ResetReason};
use super::format::format_line;
use super::state::{
    Cursor, CursorResponse, Format, Query, QueryResponse, ScreenResponse, ScrollbackResponse,
};

pub async fn run(
    mut raw_rx: broadcast::Receiver<Bytes>,
    mut query_rx: mpsc::Receiver<(Query, oneshot::Sender<QueryResponse>)>,
    event_tx: broadcast::Sender<Event>,
    cols: usize,
    rows: usize,
    scrollback_limit: usize,
) {
    let mut vt = avt::Vt::builder()
        .size(cols, rows)
        .scrollback_limit(scrollback_limit)
        .build();

    let mut seq: u64 = 0;
    let epoch: u64 = 0;
    let mut last_cursor = vt.cursor();

    loop {
        tokio::select! {
            result = raw_rx.recv() => {
                match result {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        let changes = vt.feed_str(&text);

                        // Extract changed line indices before dropping the Changes struct
                        // (Changes contains a reference to vt via its scrollback iterator)
                        let changed_lines: Vec<usize> = changes.lines.clone();
                        drop(changes);

                        // Emit line events for changed lines
                        let total_lines = vt.lines().count();
                        for line_idx in changed_lines {
                            if let Some(line) = vt.lines().nth(line_idx) {
                                seq += 1;
                                let _ = event_tx.send(Event::Line {
                                    seq,
                                    index: line_idx,
                                    total_lines,
                                    line: format_line(line, true),
                                });
                            }
                        }

                        // Emit cursor event if changed
                        let cursor = vt.cursor();
                        if cursor.row != last_cursor.row
                            || cursor.col != last_cursor.col
                            || cursor.visible != last_cursor.visible
                        {
                            seq += 1;
                            let _ = event_tx.send(Event::Cursor {
                                seq,
                                row: cursor.row,
                                col: cursor.col,
                                visible: cursor.visible,
                            });
                            last_cursor = cursor;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(n, "parser lagged, some output may be lost");
                        continue;
                    }
                }
            }

            Some((query, response_tx)) = query_rx.recv() => {
                let response = handle_query(&mut vt, query, epoch, &mut seq, &event_tx);
                let _ = response_tx.send(response);
            }
        }
    }
}

fn handle_query(
    vt: &mut avt::Vt,
    query: Query,
    epoch: u64,
    seq: &mut u64,
    event_tx: &broadcast::Sender<Event>,
) -> QueryResponse {
    match query {
        Query::Screen { format } => {
            let styled = matches!(format, Format::Styled);
            let (cols, rows) = vt.size();
            let cursor = vt.cursor();

            let total_lines = vt.lines().count();
            let first_line_index = total_lines.saturating_sub(rows);
            let lines: Vec<_> = vt.view().map(|l| format_line(l, styled)).collect();

            QueryResponse::Screen(ScreenResponse {
                epoch,
                first_line_index,
                total_lines,
                lines,
                cursor: Cursor {
                    row: cursor.row,
                    col: cursor.col,
                    visible: cursor.visible,
                },
                cols,
                rows,
                alternate_active: false, // avt doesn't expose this directly
            })
        }

        Query::Scrollback {
            format,
            offset,
            limit,
        } => {
            let styled = matches!(format, Format::Styled);
            let all_lines: Vec<_> = vt.lines().collect();
            let (_, rows) = vt.size();

            let scrollback_count = all_lines.len().saturating_sub(rows);
            let scrollback_lines: Vec<_> = all_lines
                .into_iter()
                .take(scrollback_count)
                .skip(offset)
                .take(limit)
                .map(|l| format_line(l, styled))
                .collect();

            QueryResponse::Scrollback(ScrollbackResponse {
                epoch,
                lines: scrollback_lines,
                total_lines: scrollback_count,
                offset,
            })
        }

        Query::Cursor => {
            let cursor = vt.cursor();
            QueryResponse::Cursor(CursorResponse {
                epoch,
                cursor: Cursor {
                    row: cursor.row,
                    col: cursor.col,
                    visible: cursor.visible,
                },
            })
        }

        Query::Resize { cols, rows } => {
            let _changes = vt.resize(cols, rows);
            *seq += 1;
            let _ = event_tx.send(Event::Reset {
                seq: *seq,
                reason: ResetReason::Resize,
            });
            QueryResponse::Ok
        }
    }
}
