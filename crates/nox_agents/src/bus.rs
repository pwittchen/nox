//! Per-workspace message bus and activity log (SPEC 7.13 "Messages", 7.12 "Activity").
//!
//! The bus keeps the last [`MAX_MESSAGES`] messages in memory and, when opened with
//! [`MessageBus::load`], in `~/.nox/workspaces/<id>/messages.jsonl`. The file is append-only JSON
//! lines: one line per [`Message`] when it is posted, plus `{"read":[ids],"pane":p}` lines when a
//! pane reads messages; it is compacted (rewritten atomically) when it grows past twice the cap.
//! Persistence failures are logged and never lose the in-memory state.
//!
//! `nox inbox --wait` uses [`MessageBus::subscribe`]: subscribe first, then check
//! [`MessageBus::inbox`]; if it is empty, block on the receiver (no polling). Delivery to a
//! subscriber doesn't mark the message read — call `inbox` after waking up.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Result;

/// Messages kept per workspace.
pub const MAX_MESSAGES: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(pub u64);

/// The sending pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRef {
    pub id: u64,
    pub name: String,
}

impl PaneRef {
    pub fn new(id: u64, name: impl Into<String>) -> Self {
        Self { id, name: name.into() }
    }
}

/// Serialized as the pane id (number) or `"all"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Recipient {
    Pane(u64),
    /// Every pane of the workspace except the sender.
    All,
}

impl Serialize for Recipient {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Recipient::Pane(id) => s.serialize_u64(*id),
            Recipient::All => s.serialize_str("all"),
        }
    }
}

impl<'de> Deserialize<'de> for Recipient {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Id(u64),
            Str(String),
        }
        match Repr::deserialize(d)? {
            Repr::Id(id) => Ok(Recipient::Pane(id)),
            Repr::Str(s) if s == "all" => Ok(Recipient::All),
            Repr::Str(s) => s
                .parse()
                .map(Recipient::Pane)
                .map_err(|_| serde::de::Error::custom(format!("invalid recipient {s:?}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub id: MessageId,
    pub from: u64,
    pub from_name: String,
    pub to: Recipient,
    pub text: String,
    /// Unix time in milliseconds.
    pub ts: u64,
    /// Panes that have read it (for `All` several).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_by: Vec<u64>,
}

impl Message {
    /// Is `pane` a recipient?
    #[must_use]
    pub fn is_for(&self, pane: u64) -> bool {
        match self.to {
            Recipient::Pane(p) => p == pane,
            Recipient::All => self.from != pane,
        }
    }

    #[must_use]
    pub fn is_read_by(&self, pane: u64) -> bool {
        self.read_by.contains(&pane)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Line {
    Read { read: Vec<MessageId>, pane: u64 },
    Msg(Message),
}

fn unix_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

#[derive(Debug, Default)]
pub struct MessageBus {
    messages: VecDeque<Message>,
    next_id: u64,
    path: Option<PathBuf>,
    lines_in_file: usize,
    subscribers: Vec<(u64, Sender<Message>)>,
}

impl MessageBus {
    /// An in-memory bus.
    #[must_use]
    pub fn new() -> Self {
        Self { next_id: 1, ..Default::default() }
    }

    /// Opens the bus persisted at `path` (`messages.jsonl`), loading what is there; a missing
    /// file is an empty bus. Malformed lines are skipped (logged). Later posts append to it.
    pub fn load(path: &Path) -> Result<Self> {
        let mut bus = Self::new();
        bus.path = Some(path.to_path_buf());
        let file = match File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(bus),
            Err(e) => return Err(e.into()),
        };
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            bus.lines_in_file += 1;
            match serde_json::from_str::<Line>(&line) {
                Ok(Line::Msg(m)) => {
                    bus.next_id = bus.next_id.max(m.id.0 + 1);
                    bus.messages.push_back(m);
                    if bus.messages.len() > MAX_MESSAGES {
                        bus.messages.pop_front();
                    }
                }
                Ok(Line::Read { read, pane }) => bus.apply_read(&read, pane),
                Err(e) => log::warn!("{}: skipping malformed line: {e}", path.display()),
            }
        }
        Ok(bus)
    }

    fn apply_read(&mut self, ids: &[MessageId], pane: u64) {
        for m in self.messages.iter_mut().filter(|m| ids.contains(&m.id)) {
            if !m.read_by.contains(&pane) {
                m.read_by.push(pane);
            }
        }
    }

    fn append(&mut self, line: &Line) {
        let Some(path) = self.path.clone() else {
            return;
        };
        if self.lines_in_file >= 2 * MAX_MESSAGES {
            // The in-memory state already includes this line's effect.
            match self.compact(&path) {
                Ok(()) => return,
                Err(e) => log::warn!("{}: cannot compact: {e}", path.display()),
            }
        }
        let res = (|| -> Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let mut f = OpenOptions::new().create(true).append(true).open(&path)?;
            let mut buf = serde_json::to_vec(line)?;
            buf.push(b'\n');
            f.write_all(&buf)?;
            Ok(())
        })();
        match res {
            Ok(()) => self.lines_in_file += 1,
            Err(e) => log::warn!("{}: cannot append message: {e}", path.display()),
        }
    }

    /// Rewrites the file with exactly the messages in memory (atomic: temp file + rename).
    fn compact(&mut self, path: &Path) -> Result<()> {
        let dir = path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
        for m in &self.messages {
            serde_json::to_writer(&mut tmp, m)?;
            tmp.write_all(b"\n")?;
        }
        tmp.as_file().sync_all()?;
        tmp.persist(path).map_err(|e| e.error)?;
        self.lines_in_file = self.messages.len();
        Ok(())
    }

    /// Posts a message and wakes the recipients' waiters.
    pub fn post(
        &mut self,
        from: &PaneRef,
        to: Recipient,
        text: impl Into<String>,
        now: SystemTime,
    ) -> MessageId {
        let id = MessageId(self.next_id);
        self.next_id += 1;
        let msg = Message {
            id,
            from: from.id,
            from_name: from.name.clone(),
            to,
            text: text.into(),
            ts: unix_ms(now),
            read_by: Vec::new(),
        };
        self.messages.push_back(msg.clone());
        while self.messages.len() > MAX_MESSAGES {
            self.messages.pop_front();
        }
        self.subscribers.retain(|(pane, tx)| !msg.is_for(*pane) || tx.send(msg.clone()).is_ok());
        self.append(&Line::Msg(msg));
        id
    }

    /// Unread messages of `pane` (optionally only from pane `from`), oldest first. Marks them
    /// read unless `peek`.
    pub fn inbox(&mut self, pane: u64, from: Option<u64>, peek: bool) -> Vec<Message> {
        let out: Vec<Message> = self
            .messages
            .iter()
            .filter(|m| m.is_for(pane) && !m.is_read_by(pane) && from.is_none_or(|f| m.from == f))
            .cloned()
            .collect();
        if !peek && !out.is_empty() {
            let ids: Vec<MessageId> = out.iter().map(|m| m.id).collect();
            self.apply_read(&ids, pane);
            self.append(&Line::Read { read: ids, pane });
        }
        out
    }

    #[must_use]
    pub fn unread_count(&self, pane: u64) -> usize {
        self.messages.iter().filter(|m| m.is_for(pane) && !m.is_read_by(pane)).count()
    }

    /// Every kept message, oldest first.
    pub fn messages(&self) -> impl DoubleEndedIterator<Item = &Message> {
        self.messages.iter()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// A channel receiving every message posted to `pane` from now on. Dropping the receiver
    /// unsubscribes (pruned on the next post).
    pub fn subscribe(&mut self, pane: u64) -> Receiver<Message> {
        let (tx, rx) = channel();
        self.subscribers.push((pane, tx));
        rx
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityKind {
    Message,
    Send,
    Spawn,
    Claim,
    Release,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEntry {
    pub ts: SystemTime,
    pub kind: ActivityKind,
    /// Name of the pane that did it (or `you` for the user).
    pub source_name: String,
    pub detail: String,
}

/// Bounded cooperation log of a workspace (the Activity tab of the agents panel).
#[derive(Debug, Clone)]
pub struct ActivityLog {
    entries: VecDeque<ActivityEntry>,
    cap: usize,
}

impl Default for ActivityLog {
    fn default() -> Self {
        Self::new(Self::DEFAULT_CAP)
    }
}

impl ActivityLog {
    pub const DEFAULT_CAP: usize = 500;

    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self { entries: VecDeque::with_capacity(cap.min(1024)), cap: cap.max(1) }
    }

    pub fn push(&mut self, entry: ActivityEntry) {
        if self.entries.len() == self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }

    pub fn record(
        &mut self,
        kind: ActivityKind,
        source_name: impl Into<String>,
        detail: impl Into<String>,
        now: SystemTime,
    ) {
        self.push(ActivityEntry { ts: now, kind, source_name: source_name.into(), detail: detail.into() });
    }

    /// Newest first.
    pub fn newest_first(&self) -> impl Iterator<Item = &ActivityEntry> {
        self.entries.iter().rev()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn t(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(1_700_000_000_000 + ms)
    }

    #[test]
    fn ordering_inbox_and_broadcast() {
        let mut bus = MessageBus::new();
        let a = PaneRef::new(1, "backend");
        let b = PaneRef::new(2, "reviewer");
        let c = PaneRef::new(3, "docs");
        let m1 = bus.post(&a, Recipient::Pane(2), "first", t(1));
        let m2 = bus.post(&c, Recipient::Pane(2), "second", t(2));
        let m3 = bus.post(&a, Recipient::All, "everyone", t(3));
        assert!(m1 < m2 && m2 < m3);
        assert_eq!(bus.unread_count(2), 3);
        assert_eq!(bus.unread_count(1), 0, "a broadcast skips its sender");
        assert_eq!(bus.unread_count(3), 1);
        let peek = bus.inbox(2, None, true);
        assert_eq!(peek.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["first", "second", "everyone"]);
        assert_eq!(bus.unread_count(2), 3, "peek doesn't mark read");
        let from_a = bus.inbox(2, Some(1), false);
        assert_eq!(from_a.iter().map(|m| m.id).collect::<Vec<_>>(), [m1, m3]);
        assert_eq!(from_a[0].from_name, "backend");
        assert_eq!(from_a[0].ts, 1_700_000_000_001);
        assert_eq!(bus.unread_count(2), 1);
        assert_eq!(bus.inbox(2, None, false).len(), 1);
        assert!(bus.inbox(2, None, false).is_empty());
        // Pane 3 still has the broadcast unread.
        assert_eq!(bus.inbox(3, None, false)[0].id, m3);
        let _ = b;
    }

    #[test]
    fn waiters_are_woken_without_polling() {
        let mut bus = MessageBus::new();
        let rx = bus.subscribe(2);
        let other = bus.subscribe(5);
        let dropped = bus.subscribe(2);
        drop(dropped);
        bus.post(&PaneRef::new(1, "a"), Recipient::Pane(2), "hi", t(0));
        assert_eq!(rx.recv_timeout(Duration::from_secs(1)).unwrap().text, "hi");
        assert!(other.try_recv().is_err());
        bus.post(&PaneRef::new(1, "a"), Recipient::All, "all", t(1));
        assert_eq!(rx.try_recv().unwrap().text, "all");
        assert_eq!(other.try_recv().unwrap().text, "all");
        assert_eq!(bus.subscribers.len(), 2, "dropped receivers are pruned");
        // Delivery doesn't mark read.
        assert_eq!(bus.unread_count(2), 2);
    }

    #[test]
    fn waiter_on_another_thread() {
        let bus = std::sync::Arc::new(std::sync::Mutex::new(MessageBus::new()));
        let rx = bus.lock().unwrap().subscribe(7);
        let h = std::thread::spawn(move || rx.recv().unwrap().text);
        bus.lock().unwrap().post(&PaneRef::new(1, "a"), Recipient::Pane(7), "wake", t(0));
        assert_eq!(h.join().unwrap(), "wake");
    }

    #[test]
    fn persistence_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws/messages.jsonl");
        {
            let mut bus = MessageBus::load(&path).unwrap();
            bus.post(&PaneRef::new(1, "a"), Recipient::Pane(2), "one", t(1));
            bus.post(&PaneRef::new(2, "b"), Recipient::All, "two \"quoted\"\nline", t(2));
            bus.inbox(2, None, false);
        }
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 3);
        assert!(raw.contains("\"to\":\"all\""));
        let mut bus = MessageBus::load(&path).unwrap();
        assert_eq!(bus.len(), 2);
        assert_eq!(bus.unread_count(2), 0);
        assert_eq!(bus.unread_count(1), 1);
        let m = bus.inbox(1, None, false);
        assert_eq!(m[0].text, "two \"quoted\"\nline");
        assert_eq!(m[0].to, Recipient::All);
        // Ids continue after a reload.
        let id = bus.post(&PaneRef::new(1, "a"), Recipient::Pane(2), "three", t(3));
        assert_eq!(id, MessageId(3));
        // A malformed line is skipped.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{{not json").unwrap();
        assert_eq!(MessageBus::load(&path).unwrap().len(), 3);
    }

    #[test]
    fn cap_and_compaction() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("messages.jsonl");
        let mut bus = MessageBus::load(&path).unwrap();
        let from = PaneRef::new(1, "a");
        for i in 0..2_500u64 {
            bus.post(&from, Recipient::Pane(2), format!("m{i}"), t(i));
        }
        assert_eq!(bus.len(), MAX_MESSAGES);
        assert_eq!(bus.messages().next().unwrap().text, "m1500");
        let lines = std::fs::read_to_string(&path).unwrap().lines().count();
        assert!(lines <= 2 * MAX_MESSAGES, "file is compacted ({lines} lines)");
        let reloaded = MessageBus::load(&path).unwrap();
        assert_eq!(reloaded.len(), MAX_MESSAGES);
        assert_eq!(reloaded.messages().next().unwrap().text, "m1500");
        assert_eq!(reloaded.messages().last().unwrap().text, "m2499");
    }

    #[test]
    fn activity_log_is_bounded_newest_first() {
        let mut log = ActivityLog::new(3);
        for i in 0..5 {
            log.record(ActivityKind::Message, "a", format!("{i}"), t(i));
        }
        assert_eq!(log.len(), 3);
        let details: Vec<_> = log.newest_first().map(|e| e.detail.as_str()).collect();
        assert_eq!(details, ["4", "3", "2"]);
    }
}
