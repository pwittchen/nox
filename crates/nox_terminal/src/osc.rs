//! OSC sniffer (SPEC §7.1, §8).
//!
//! A tiny state machine that watches the PTY output stream for the OSC sequences nox handles
//! itself — OSC 7 (cwd), OSC 133 (prompt marks), OSC 9 / OSC 777 (notifications) — without
//! copying or altering the stream. The bytes still go to the VT parser unchanged (which ignores
//! these sequences). Sequences may be split across reads; state carries over between calls.

use std::path::PathBuf;

/// Something the sniffer recognized. `end` is the offset in the fed chunk just past the
/// sequence terminator, so the reader can parse up to that point before acting on it (e.g. to
/// record the cursor line of a prompt mark).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OscHit {
    pub end: usize,
    pub event: OscEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    /// OSC 7: `file://host/path` — the shell's working directory.
    Cwd(PathBuf),
    /// OSC 133;A — prompt start.
    PromptStart,
    /// OSC 133;B — command input start (end of prompt).
    InputStart,
    /// OSC 133;C — command output start (the command is running).
    CommandStart,
    /// OSC 133;D[;exit] — command finished.
    CommandEnd(Option<i32>),
    /// OSC 9 (body only) or OSC 777;notify;title;body.
    Notify { title: Option<String>, body: String },
}

/// Max payload kept for one OSC; longer ones (e.g. OSC 52 with a big clipboard) are skipped.
const MAX_PAYLOAD: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Esc,
    /// Inside an OSC; collecting the payload.
    Osc,
    /// Saw ESC inside an OSC (possible ST = ESC \).
    OscEsc,
}

#[derive(Debug)]
pub struct OscSniffer {
    state: State,
    buf: Vec<u8>,
    overflow: bool,
}

impl Default for OscSniffer {
    fn default() -> Self {
        Self::new()
    }
}

impl OscSniffer {
    #[must_use]
    pub fn new() -> Self {
        Self { state: State::Ground, buf: Vec::with_capacity(256), overflow: false }
    }

    /// Feed a chunk of PTY output; returns recognized sequences in order.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<OscHit> {
        let mut hits = Vec::new();
        // Fast path: nothing pending and no ESC / C1 OSC in the chunk.
        if self.state == State::Ground && !bytes.iter().any(|&b| b == 0x1b || b == 0x9d) {
            return hits;
        }
        for (i, &b) in bytes.iter().enumerate() {
            match self.state {
                State::Ground => {
                    if b == 0x1b {
                        self.state = State::Esc;
                    } else if b == 0x9d {
                        self.start_osc();
                    }
                }
                State::Esc => {
                    if b == b']' {
                        self.start_osc();
                    } else if b == 0x1b {
                        self.state = State::Esc;
                    } else {
                        self.state = State::Ground;
                    }
                }
                State::Osc => match b {
                    0x07 | 0x9c => self.finish(i + 1, &mut hits),
                    0x1b => self.state = State::OscEsc,
                    // CAN / SUB abort the sequence.
                    0x18 | 0x1a => self.state = State::Ground,
                    _ => self.push(b),
                },
                State::OscEsc => {
                    if b == b'\\' {
                        self.finish(i + 1, &mut hits);
                    } else if b == b']' {
                        // ESC ] inside an OSC: abort and start a new one.
                        self.start_osc();
                    } else {
                        self.state = State::Ground;
                    }
                }
            }
        }
        hits
    }

    fn start_osc(&mut self) {
        self.state = State::Osc;
        self.buf.clear();
        self.overflow = false;
    }

    fn push(&mut self, b: u8) {
        if self.buf.len() < MAX_PAYLOAD {
            self.buf.push(b);
        } else {
            self.overflow = true;
        }
    }

    fn finish(&mut self, end: usize, hits: &mut Vec<OscHit>) {
        self.state = State::Ground;
        if self.overflow {
            return;
        }
        if let Some(event) = parse_payload(&self.buf) {
            hits.push(OscHit { end, event });
        }
    }
}

fn parse_payload(payload: &[u8]) -> Option<OscEvent> {
    let text = String::from_utf8_lossy(payload);
    let (code, rest) = match text.split_once(';') {
        Some((c, r)) => (c, r),
        None => (text.as_ref(), ""),
    };
    match code {
        "7" => parse_osc7(rest).map(OscEvent::Cwd),
        "133" => {
            let mut parts = rest.split(';');
            match parts.next()? {
                "A" => Some(OscEvent::PromptStart),
                "B" => Some(OscEvent::InputStart),
                "C" => Some(OscEvent::CommandStart),
                "D" => {
                    let code = parts.next().and_then(|s| s.trim().parse::<i32>().ok());
                    Some(OscEvent::CommandEnd(code))
                }
                _ => None,
            }
        }
        "9" => {
            // OSC 9;4;… is ConEmu progress, not a notification.
            if rest.starts_with("4;") || rest.is_empty() {
                return None;
            }
            Some(OscEvent::Notify { title: None, body: rest.to_string() })
        }
        "777" => {
            let mut parts = rest.splitn(3, ';');
            if parts.next()? != "notify" {
                return None;
            }
            let title = parts.next().unwrap_or("").to_string();
            let body = parts.next().unwrap_or("").to_string();
            Some(OscEvent::Notify { title: (!title.is_empty()).then_some(title), body })
        }
        _ => None,
    }
}

fn parse_osc7(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://").or_else(|| uri.strip_prefix("kitty-shell-cwd://"))?;
    // Skip the host part.
    let path = {
        let i = rest.find('/')?;
        &rest[i..]
    };
    Some(PathBuf::from(percent_decode(path)))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(s: &mut OscSniffer, chunks: &[&[u8]]) -> Vec<OscEvent> {
        chunks.iter().flat_map(|c| s.feed(c)).map(|h| h.event).collect()
    }

    #[test]
    fn osc7_bel_and_st() {
        let mut s = OscSniffer::new();
        let ev = events(&mut s, &[b"hi\x1b]7;file://mbp/Users/pw/My%20Dir\x07x\x1b]7;file://h/tmp\x1b\\"]);
        assert_eq!(
            ev,
            vec![OscEvent::Cwd(PathBuf::from("/Users/pw/My Dir")), OscEvent::Cwd(PathBuf::from("/tmp"))]
        );
    }

    #[test]
    fn split_across_reads() {
        let mut s = OscSniffer::new();
        let seq = b"\x1b]133;D;1\x1b\\";
        for split in 1..seq.len() {
            let mut s2 = OscSniffer::new();
            let ev = events(&mut s2, &[&seq[..split], &seq[split..]]);
            assert_eq!(ev, vec![OscEvent::CommandEnd(Some(1))], "split at {split}");
        }
        assert!(s.feed(b"plain text").is_empty());
    }

    #[test]
    fn prompt_marks() {
        let mut s = OscSniffer::new();
        let ev = events(&mut s, &[b"\x1b]133;A\x07$ \x1b]133;B\x07ls\r\n\x1b]133;C\x07out\x1b]133;D\x07"]);
        assert_eq!(
            ev,
            vec![
                OscEvent::PromptStart,
                OscEvent::InputStart,
                OscEvent::CommandStart,
                OscEvent::CommandEnd(None)
            ]
        );
    }

    #[test]
    fn notifications() {
        let mut s = OscSniffer::new();
        let ev = events(
            &mut s,
            &[b"\x1b]9;Claude needs your permission\x07\x1b]777;notify;Build;done ok\x07\x1b]9;4;1;50\x07"],
        );
        assert_eq!(
            ev,
            vec![
                OscEvent::Notify { title: None, body: "Claude needs your permission".into() },
                OscEvent::Notify { title: Some("Build".into()), body: "done ok".into() },
            ]
        );
    }

    #[test]
    fn hit_offsets_point_past_terminator() {
        let mut s = OscSniffer::new();
        let data = b"ab\x1b]133;C\x07cd";
        let hits = s.feed(data);
        assert_eq!(hits.len(), 1);
        assert_eq!(&data[hits[0].end..], b"cd");
    }

    #[test]
    fn ignores_other_osc_and_oversized() {
        let mut s = OscSniffer::new();
        assert!(s.feed(b"\x1b]0;title\x07\x1b]2;x\x1b\\").is_empty());
        let mut big = b"\x1b]9;".to_vec();
        big.extend(std::iter::repeat_n(b'a', MAX_PAYLOAD + 10));
        big.push(7);
        assert!(s.feed(&big).is_empty());
        // Recovers afterwards.
        assert_eq!(s.feed(b"\x1b]133;A\x07").len(), 1);
    }
}
