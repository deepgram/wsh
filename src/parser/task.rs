use bytes::Bytes;
use tokio::sync::{broadcast, mpsc, oneshot};

use super::events::{Event, ResetReason};
use super::format::format_line;
use super::state::{
    Cursor, CursorResponse, Format, Query, QueryResponse, ScreenResponse, ScrollbackResponse,
};

pub async fn run(
    mut raw_rx: mpsc::UnboundedReceiver<Bytes>,
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
    let mut alternate_active = false;
    let mut alt_detect = AlternateScreenDetector::new();

    loop {
        tokio::select! {
            result = raw_rx.recv() => {
                match result {
                    Some(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);

                        // Detect alternate screen transitions before feeding to avt
                        let new_alternate = alt_detect.feed(&text, alternate_active);

                        let changes = vt.feed_str(&text);

                        // Extract changed line indices before dropping the Changes struct
                        // (Changes contains a reference to vt via its scrollback iterator)
                        let changed_lines: Vec<usize> = changes.lines.clone();
                        drop(changes);

                        // Emit mode/reset events if alternate screen state changed
                        if new_alternate != alternate_active {
                            alternate_active = new_alternate;
                            seq += 1;
                            let _ = event_tx.send(Event::Mode {
                                seq,
                                alternate_active,
                            });
                            seq += 1;
                            let _ = event_tx.send(Event::Reset {
                                seq,
                                reason: if alternate_active {
                                    ResetReason::AlternateScreenEnter
                                } else {
                                    ResetReason::AlternateScreenExit
                                },
                            });
                        }

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
                    None => break,
                }
            }

            Some((query, response_tx)) = query_rx.recv() => {
                let response = handle_query(&mut vt, query, epoch, alternate_active, &mut seq, &event_tx);
                let _ = response_tx.send(response);
            }
        }
    }
}

fn handle_query(
    vt: &mut avt::Vt,
    query: Query,
    epoch: u64,
    alternate_active: bool,
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
                alternate_active,
            })
        }

        Query::Scrollback {
            format,
            offset,
            limit,
        } => {
            let styled = matches!(format, Format::Styled);
            let all_lines: Vec<_> = vt.lines().collect();
            let total_lines = all_lines.len();

            // Return all lines (history + current screen), applying offset/limit
            // In alternate screen mode, this returns just the current screen
            // since the alternate buffer has no scrollback history
            let lines: Vec<_> = all_lines
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|l| format_line(l, styled))
                .collect();

            QueryResponse::Scrollback(ScrollbackResponse {
                epoch,
                lines,
                total_lines,
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

/// Stateful detector for alternate screen mode transitions.
///
/// Tracks DEC private mode set/reset sequences (modes 47, 1047, 1049) across
/// chunk boundaries. Terminal output arrives in arbitrary-sized chunks that may
/// split an escape sequence (e.g. `\x1b` in one chunk, `[?1049h` in the next).
/// This detector buffers partial sequences to handle such splits correctly.
struct AlternateScreenDetector {
    /// Partial CSI sequence carried over from previous chunk.
    /// Contains bytes from ESC or CSI introducer through any partial params.
    partial: Vec<u8>,
}

/// Internal states while scanning a byte within the detector.
#[derive(Clone, Copy)]
enum ScanState {
    /// Not inside any escape sequence
    Ground,
    /// Seen ESC (0x1b), waiting for '['
    Esc,
    /// Inside CSI, waiting for '?' or skipping non-DEC CSI
    CsiEntry,
    /// Seen CSI ?, collecting parameter bytes
    DecParams,
}

impl AlternateScreenDetector {
    fn new() -> Self {
        Self {
            partial: Vec::new(),
        }
    }

    /// Feed a chunk of text and return the new alternate_active state.
    fn feed(&mut self, text: &str, current: bool) -> bool {
        let mut state = current;
        let mut scan = if self.partial.is_empty() {
            ScanState::Ground
        } else {
            // Determine where we left off from the partial buffer
            self.classify_partial()
        };

        for &byte in text.as_bytes() {
            match scan {
                ScanState::Ground => {
                    if byte == 0x1b {
                        self.partial.clear();
                        self.partial.push(byte);
                        scan = ScanState::Esc;
                    } else if byte == 0xC2 {
                        // Potential start of C1 CSI (U+009B = 0xC2 0x9B in UTF-8)
                        self.partial.clear();
                        self.partial.push(byte);
                        // Stay in ground — we'll check next byte
                    } else if !self.partial.is_empty() && self.partial[0] == 0xC2 && byte == 0x9B {
                        // Completed C1 CSI
                        self.partial.clear();
                        self.partial.push(0xC2);
                        self.partial.push(0x9B);
                        scan = ScanState::CsiEntry;
                    } else {
                        self.partial.clear();
                    }
                }

                ScanState::Esc => {
                    if byte == b'[' {
                        self.partial.push(byte);
                        scan = ScanState::CsiEntry;
                    } else {
                        // Not a CSI, abandon
                        self.partial.clear();
                        scan = ScanState::Ground;
                    }
                }

                ScanState::CsiEntry => {
                    if byte == b'?' {
                        self.partial.push(byte);
                        scan = ScanState::DecParams;
                    } else {
                        // Not a DEC private mode sequence, abandon
                        self.partial.clear();
                        scan = ScanState::Ground;
                        // Re-check this byte as potential sequence start
                        if byte == 0x1b {
                            self.partial.push(byte);
                            scan = ScanState::Esc;
                        }
                    }
                }

                ScanState::DecParams => {
                    if byte >= 0x30 && byte <= 0x3f {
                        // Parameter byte (digits, semicolons, etc.)
                        self.partial.push(byte);
                    } else if byte == b'h' || byte == b'l' {
                        // Final byte — process the complete sequence
                        let entering = byte == b'h';
                        if let Some(new_state) = self.process_params(entering) {
                            state = new_state;
                        }
                        self.partial.clear();
                        scan = ScanState::Ground;
                    } else {
                        // Unexpected byte, not a valid DEC mode sequence
                        self.partial.clear();
                        scan = ScanState::Ground;
                        if byte == 0x1b {
                            self.partial.push(byte);
                            scan = ScanState::Esc;
                        }
                    }
                }
            }
        }

        // partial remains populated for next call if we're mid-sequence
        if matches!(scan, ScanState::Ground) {
            self.partial.clear();
        }

        state
    }

    /// Classify what scan state the existing partial buffer represents.
    fn classify_partial(&self) -> ScanState {
        let p = &self.partial;
        if p.is_empty() {
            return ScanState::Ground;
        }

        // Check for C1 CSI partial (0xC2 alone — waiting for 0x9B)
        if p.len() == 1 && p[0] == 0xC2 {
            return ScanState::Ground; // handled specially in Ground
        }

        // ESC alone
        if p.len() == 1 && p[0] == 0x1b {
            return ScanState::Esc;
        }

        // ESC [ or C1 CSI (0xC2 0x9B)
        let csi = (p.len() >= 2 && p[0] == 0x1b && p[1] == b'[')
            || (p.len() >= 2 && p[0] == 0xC2 && p[1] == 0x9B);

        if !csi {
            return ScanState::Ground;
        }

        // Check if we have the '?' yet
        let after_csi = if p[0] == 0x1b { 2 } else { 2 };
        if p.len() <= after_csi {
            return ScanState::CsiEntry;
        }

        if p[after_csi] == b'?' {
            ScanState::DecParams
        } else {
            ScanState::Ground
        }
    }

    /// Extract params from partial buffer and check for alternate screen modes.
    /// Returns Some(bool) if an alternate screen mode was found.
    fn process_params(&self, entering: bool) -> Option<bool> {
        // Find where params start (after the '?')
        let params_start = if self.partial[0] == 0x1b {
            3 // ESC [ ?
        } else {
            3 // 0xC2 0x9B ?
        };

        if params_start > self.partial.len() {
            return None;
        }

        let params = &self.partial[params_start..];
        let params_str = std::str::from_utf8(params).ok()?;

        let mut found = false;
        for param in params_str.split(';') {
            match param {
                "47" | "1047" | "1049" => {
                    found = true;
                }
                _ => {}
            }
        }

        if found {
            Some(entering)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AlternateScreenDetector;

    fn detect(text: &str, current: bool) -> bool {
        AlternateScreenDetector::new().feed(text, current)
    }

    #[test]
    fn no_sequences_preserves_state() {
        assert!(!detect("hello world", false));
        assert!(detect("hello world", true));
    }

    #[test]
    fn decset_1049_enters_alternate() {
        assert!(detect("\x1b[?1049h", false));
    }

    #[test]
    fn decrst_1049_exits_alternate() {
        assert!(!detect("\x1b[?1049l", true));
    }

    #[test]
    fn decset_1047_enters_alternate() {
        assert!(detect("\x1b[?1047h", false));
    }

    #[test]
    fn decrst_1047_exits_alternate() {
        assert!(!detect("\x1b[?1047l", true));
    }

    #[test]
    fn decset_47_enters_alternate() {
        assert!(detect("\x1b[?47h", false));
    }

    #[test]
    fn decrst_47_exits_alternate() {
        assert!(!detect("\x1b[?47l", true));
    }

    #[test]
    fn combined_modes_with_alternate() {
        assert!(detect("\x1b[?6;1049h", false));
    }

    #[test]
    fn enter_then_exit_in_same_chunk() {
        let text = "\x1b[?1049h some output \x1b[?1049l";
        assert!(!detect(text, false));
    }

    #[test]
    fn exit_then_enter_in_same_chunk() {
        let text = "\x1b[?1049l some output \x1b[?1049h";
        assert!(detect(text, true));
    }

    #[test]
    fn c1_csi_enters_alternate() {
        assert!(detect("\u{9b}?1049h", false));
    }

    #[test]
    fn c1_csi_exits_alternate() {
        assert!(!detect("\u{9b}?1049l", true));
    }

    #[test]
    fn unrelated_dec_modes_ignored() {
        assert!(!detect("\x1b[?25h", false));
        assert!(detect("\x1b[?25l", true));
    }

    #[test]
    fn non_dec_csi_sequences_ignored() {
        assert!(!detect("\x1b[1049h", false));
    }

    #[test]
    fn mixed_with_normal_output() {
        assert!(detect("hello\x1b[?1049hworld", false));
    }

    #[test]
    fn incomplete_sequence_at_end() {
        assert!(!detect("\x1b[?1049", false));
    }

    // --- Split sequence tests ---

    #[test]
    fn split_after_esc() {
        let mut d = AlternateScreenDetector::new();
        let state = d.feed("text\x1b", false);
        assert!(!state, "ESC alone should not change state");
        let state = d.feed("[?1049h", state);
        assert!(state, "completing the sequence should enter alternate");
    }

    #[test]
    fn split_after_esc_bracket() {
        let mut d = AlternateScreenDetector::new();
        let state = d.feed("\x1b[", false);
        assert!(!state);
        let state = d.feed("?1049h", state);
        assert!(state);
    }

    #[test]
    fn split_after_question_mark() {
        let mut d = AlternateScreenDetector::new();
        let state = d.feed("\x1b[?", false);
        assert!(!state);
        let state = d.feed("1049h", state);
        assert!(state);
    }

    #[test]
    fn split_mid_params() {
        let mut d = AlternateScreenDetector::new();
        let state = d.feed("\x1b[?10", false);
        assert!(!state);
        let state = d.feed("49h", state);
        assert!(state);
    }

    #[test]
    fn split_before_final_byte() {
        let mut d = AlternateScreenDetector::new();
        let state = d.feed("\x1b[?1049", false);
        assert!(!state);
        let state = d.feed("h", state);
        assert!(state);
    }

    #[test]
    fn split_exit_sequence() {
        let mut d = AlternateScreenDetector::new();
        let state = d.feed("\x1b[?10", true);
        assert!(state);
        let state = d.feed("49l", state);
        assert!(!state);
    }

    #[test]
    fn split_c1_csi() {
        let mut d = AlternateScreenDetector::new();
        // U+009B in UTF-8 is 0xC2 0x9B — the first byte alone
        let state = d.feed("\u{9b}", false);
        // C1 CSI is a single Unicode char, so it completes in one feed
        // but the rest of the sequence could be split
        let state = d.feed("?1049h", state);
        assert!(state);
    }

    #[test]
    fn split_abandoned_then_valid() {
        let mut d = AlternateScreenDetector::new();
        // Start a non-DEC CSI sequence (no '?')
        let state = d.feed("\x1b[25h", false);
        assert!(!state);
        // Now a valid alternate screen sequence
        let state = d.feed("\x1b[?1049h", state);
        assert!(state);
    }

    #[test]
    fn split_with_interleaved_data() {
        let mut d = AlternateScreenDetector::new();
        let state = d.feed("output\x1b", false);
        assert!(!state);
        let state = d.feed("[?1049hmore output", state);
        assert!(state);
    }

    #[test]
    fn multiple_splits_three_chunks() {
        let mut d = AlternateScreenDetector::new();
        let state = d.feed("\x1b", false);
        assert!(!state);
        let state = d.feed("[?", state);
        assert!(!state);
        let state = d.feed("1049h", state);
        assert!(state);
    }

    #[test]
    fn byte_at_a_time() {
        let mut d = AlternateScreenDetector::new();
        let mut state = false;
        for byte in "\x1b[?1049h".as_bytes() {
            state = d.feed(std::str::from_utf8(&[*byte]).unwrap(), state);
        }
        assert!(state);
    }
}
