use crate::output::{clean, text};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

const MAX_LINES: usize = 10_000;
const MAX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogKey {
    pub record: String,
    pub attempt: i64,
    pub log: i32,
}

#[derive(Default)]
pub struct LogBuffer {
    pub lines: VecDeque<String>,
    pub next: u64,
    pub base: u64,
    bytes: usize,
}
impl LogBuffer {
    pub fn append(&mut self, start: u64, lines: Vec<String>) {
        // Ignore stale/overlapping fetches. Offsets count server lines, not retained lines.
        if start != self.next {
            return;
        }
        for line in lines {
            let mut line = clean(&line);
            if line.len() > 64 * 1024 {
                let mut end = 64 * 1024;
                while !line.is_char_boundary(end) {
                    end -= 1;
                }
                line.truncate(end);
                line.push_str(" [line truncated]");
            }
            self.bytes += line.len();
            self.lines.push_back(line);
            self.next += 1;
            while self.lines.len() > MAX_LINES || self.bytes > MAX_BYTES {
                if let Some(line) = self.lines.pop_front() {
                    self.bytes -= line.len();
                    self.base += 1;
                }
            }
        }
    }
}

pub struct Viewer {
    pub run_id: i32,
    pub run: Value,
    pub records: Vec<Value>,
    pub selected: Option<String>,
    pub job: Option<String>,
    pub follow: bool,
    pub log_key: Option<LogKey>,
    pub log: LogBuffer,
    pub top: u64,
    pub error: String,
    pub live_status: String,
    live_logs: HashMap<String, LogBuffer>,
}
impl Viewer {
    pub fn new(run_id: i32) -> Self {
        Self {
            run_id,
            run: Value::Null,
            records: Vec::new(),
            selected: None,
            job: None,
            follow: true,
            log_key: None,
            log: LogBuffer::default(),
            top: 0,
            error: String::new(),
            live_status: "Connecting to live console".into(),
            live_logs: HashMap::new(),
        }
    }
    pub fn update(&mut self, run: Value, records: Vec<Value>) {
        self.run = run;
        for record in &records {
            let id = text(record, "id");
            if self
                .records
                .iter()
                .any(|old| text(old, "id") == id && old["attempt"] != record["attempt"])
            {
                self.live_logs.remove(id);
            }
        }
        self.records = tree_order(records);
        if self.job.is_none() {
            self.job = self
                .records
                .iter()
                .find(|r| {
                    text(r, "type").eq_ignore_ascii_case("job") && text(r, "state") == "inProgress"
                })
                .or_else(|| {
                    self.records
                        .iter()
                        .find(|r| text(r, "type").eq_ignore_ascii_case("job"))
                })
                .map(|r| text(r, "id").to_string());
        }
        if self.follow {
            let candidates = self
                .records
                .iter()
                .filter(|r| {
                    text(r, "type").eq_ignore_ascii_case("task")
                        && self.job_for(text(r, "id")) == self.job
                })
                .collect::<Vec<_>>();
            self.selected = candidates
                .iter()
                .find(|r| text(r, "state") == "inProgress")
                .or_else(|| {
                    candidates
                        .iter()
                        .rev()
                        .find(|r| text(r, "state") == "completed")
                })
                .or_else(|| candidates.first())
                .map(|r| text(r, "id").to_string())
                .or(self.selected.take());
        }
        self.reset_log_if_changed();
    }
    pub fn job_for(&self, id: &str) -> Option<String> {
        let mut id = id;
        for _ in 0..32 {
            let record = self.records.iter().find(|r| text(r, "id") == id)?;
            if text(record, "type").eq_ignore_ascii_case("job") {
                return Some(id.into());
            }
            id = text(record, "parentId");
        }
        None
    }
    pub fn selection(&self) -> usize {
        self.records
            .iter()
            .position(|r| Some(text(r, "id")) == self.selected.as_deref())
            .unwrap_or(0)
    }
    pub fn navigate(&mut self, delta: isize) {
        self.follow = false;
        if self.records.is_empty() {
            return;
        }
        let index = self
            .selection()
            .saturating_add_signed(delta)
            .min(self.records.len() - 1);
        self.selected = Some(text(&self.records[index], "id").into());
        if let Some(job) = self.job_for(text(&self.records[index], "id")) {
            self.job = Some(job);
        }
        self.reset_log_if_changed();
    }
    pub fn accept_live(&mut self, event: crate::live::Event) {
        match event {
            crate::live::Event::Status(status) => {
                // The console feed can replay a tail after reconnecting. Keep its
                // offsets separate from the authoritative uploaded-log cursor.
                if status == "Connecting to live console" {
                    self.live_logs.clear();
                }
                self.live_status = status;
            }
            crate::live::Event::Lines { record, lines } => {
                if self
                    .records
                    .iter()
                    .any(|r| text(r, "id") == record && text(r, "state") == "completed")
                {
                    return;
                }
                if !self.live_logs.contains_key(&record) && self.live_logs.len() >= 8 {
                    let evict = self
                        .live_logs
                        .keys()
                        .find(|id| Some(id.as_str()) != self.selected.as_deref())
                        .cloned();
                    if let Some(id) = evict {
                        self.live_logs.remove(&id);
                    }
                }
                let buffer = self.live_logs.entry(record).or_default();
                buffer.append(buffer.next, lines);
            }
        }
    }
    pub fn showing_live(&self) -> bool {
        self.selected
            .as_ref()
            .and_then(|id| self.live_logs.get(id))
            .is_some_and(|log| !log.lines.is_empty())
            && (self.log.lines.is_empty()
                || self
                    .records
                    .get(self.selection())
                    .is_some_and(|r| text(r, "state") != "completed"))
    }
    pub fn visible_log(&self) -> &LogBuffer {
        if self.showing_live()
            && let Some(log) = self.selected.as_ref().and_then(|id| self.live_logs.get(id))
        {
            return log;
        }
        &self.log
    }
    pub fn scroll(&mut self, delta: i64) {
        self.follow = false;
        let log = self.visible_log();
        self.top = self
            .top
            .saturating_add_signed(delta)
            .clamp(log.base, log.next.saturating_sub(1).max(log.base));
    }
    pub fn resume(&mut self) {
        self.follow = true;
        self.update(self.run.clone(), self.records.clone());
    }
    fn reset_log_if_changed(&mut self) {
        let key = self
            .records
            .iter()
            .find(|r| Some(text(r, "id")) == self.selected.as_deref())
            .and_then(|r| {
                Some(LogKey {
                    record: text(r, "id").into(),
                    attempt: r["attempt"].as_i64().unwrap_or(1),
                    log: i32::try_from(r["log"]["id"].as_i64()?)
                        .ok()
                        .filter(|id| *id > 0)?,
                })
            });
        if key != self.log_key {
            self.log_key = key;
            self.log = LogBuffer::default();
            self.top = 0;
        }
    }
    pub fn accept_log(&mut self, key: LogKey, start: u64, lines: Vec<String>) {
        if self.log_key.as_ref() == Some(&key) {
            self.log.append(start, lines);
            self.top = self.top.max(self.log.base);
        }
    }
    pub fn offset(&mut self, height: usize) -> u16 {
        let log = self.visible_log();
        let base = log.base;
        let next = log.next;
        if self.follow {
            self.top = next.saturating_sub(height as u64).max(base);
        }
        self.top = self.top.clamp(base, next.saturating_sub(1).max(base));
        self.top.saturating_sub(base).min(u16::MAX as u64) as u16
    }
}

pub fn tree_order(records: Vec<Value>) -> Vec<Value> {
    let ids: HashSet<&str> = records.iter().map(|r| text(r, "id")).collect();
    let mut children: HashMap<&str, Vec<&Value>> = HashMap::new();
    for record in &records {
        let parent = text(record, "parentId");
        children
            .entry(if ids.contains(parent) { parent } else { "" })
            .or_default()
            .push(record);
    }
    for list in children.values_mut() {
        list.sort_by_key(|r| (r["order"].as_i64().unwrap_or(i64::MAX), text(r, "id")));
    }
    fn visit(
        id: &str,
        children: &HashMap<&str, Vec<&Value>>,
        seen: &mut HashSet<String>,
        out: &mut Vec<Value>,
        depth: usize,
    ) {
        if depth > 32 {
            return;
        }
        if let Some(list) = children.get(id) {
            for record in list {
                let key = text(record, "id");
                if seen.insert(key.into()) {
                    out.push((*record).clone());
                    visit(key, children, seen, out, depth + 1);
                }
            }
        }
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    visit("", &children, &mut seen, &mut result, 0);
    // Preserve orphaned/cyclic records rather than hiding them or recursing forever.
    for record in &records {
        if seen.insert(text(record, "id").into()) {
            result.push(record.clone());
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn records() -> Vec<Value> {
        vec![
            json!({"id":"a","type":"Job","state":"inProgress","order":1}),
            json!({"id":"b","type":"Job","state":"inProgress","order":2}),
            json!({"id":"a1","type":"Task","parentId":"a","state":"inProgress","log":{"id":1}}),
            json!({"id":"b1","type":"Task","parentId":"b","state":"inProgress","log":{"id":2}}),
        ]
    }
    #[test]
    fn follows_selected_job_and_navigation_pauses() {
        let mut viewer = Viewer::new(1);
        viewer.update(Value::Null, records());
        assert_eq!(viewer.selected.as_deref(), Some("a1"));
        viewer.navigate(2);
        assert_eq!(viewer.selected.as_deref(), Some("b1"));
        assert!(!viewer.follow);
        viewer.update(Value::Null, records());
        assert_eq!(viewer.selected.as_deref(), Some("b1"));
        viewer.resume();
        assert_eq!(viewer.job.as_deref(), Some("b"));
    }
    #[test]
    fn stale_log_is_ignored_and_attempt_changes_reset() -> anyhow::Result<()> {
        use anyhow::Context;
        let mut viewer = Viewer::new(1);
        viewer.update(Value::Null, records());
        let key = viewer.log_key.clone().context("Expected task log")?;
        viewer.accept_log(key.clone(), 0, vec!["line".into()]);
        viewer.accept_log(key.clone(), 0, vec!["duplicate".into()]);
        assert_eq!(viewer.log.next, 1);
        let mut updated = records();
        updated[2]["attempt"] = json!(2);
        viewer.update(Value::Null, updated);
        assert_eq!(viewer.log.next, 0);
        viewer.accept_log(key, 0, vec!["stale".into()]);
        assert_eq!(viewer.log.next, 0);
        Ok(())
    }
    #[test]
    fn live_output_before_upload_is_replaced_by_archive_without_skipping_lines()
    -> anyhow::Result<()> {
        use anyhow::Context;
        let mut viewer = Viewer::new(1);
        let mut timeline = records();
        timeline[2]
            .as_object_mut()
            .context("Task object")?
            .remove("log");
        viewer.update(Value::Null, timeline.clone());
        assert!(viewer.log_key.is_none());
        viewer.accept_live(crate::live::Event::Lines {
            record: "a1".into(),
            lines: vec!["live one".into(), "live two".into()],
        });
        assert!(viewer.showing_live());
        assert_eq!(viewer.visible_log().lines.len(), 2);
        assert_eq!(viewer.log.next, 0);
        viewer.scroll(-1);
        assert_eq!(viewer.visible_log().lines[0], "live one");
        timeline[2]["state"] = json!("completed");
        timeline[2]["log"] = json!({"id":1});
        viewer.update(Value::Null, timeline);
        let key = viewer.log_key.clone().context("Uploaded log available")?;
        assert_eq!(viewer.log.next, 0);
        assert!(viewer.showing_live());
        viewer.accept_log(
            key,
            0,
            vec![
                "archive one".into(),
                "archive two".into(),
                "archive three".into(),
            ],
        );
        assert!(!viewer.showing_live());
        assert_eq!(viewer.visible_log().lines.len(), 3);
        assert_eq!(viewer.log.next, 3);
        Ok(())
    }

    #[test]
    fn live_tasks_are_isolated_and_retry_does_not_reuse_output() {
        let mut viewer = Viewer::new(1);
        viewer.update(Value::Null, records());
        viewer.accept_live(crate::live::Event::Lines {
            record: "a1".into(),
            lines: vec!["task a".into()],
        });
        viewer.accept_live(crate::live::Event::Lines {
            record: "b1".into(),
            lines: vec!["task b".into()],
        });
        viewer.navigate(2);
        assert_eq!(viewer.visible_log().lines[0], "task b");
        let mut timeline = records();
        timeline[3]["attempt"] = json!(2);
        viewer.update(Value::Null, timeline);
        assert!(viewer.visible_log().lines.is_empty());
        viewer.accept_live(crate::live::Event::Status(
            "Connecting to live console".into(),
        ));
        assert!(viewer.live_logs.is_empty());
        for index in 0..20 {
            viewer.accept_live(crate::live::Event::Lines {
                record: format!("task{index}"),
                lines: vec!["line".into()],
            });
        }
        assert_eq!(viewer.live_logs.len(), 8);
    }

    #[test]
    fn retention_preserves_absolute_offsets() {
        let mut log = LogBuffer::default();
        log.append(0, (0..12_000).map(|i| i.to_string()).collect());
        assert_eq!(log.next, 12_000);
        assert_eq!(log.base, 2_000);
        assert_eq!(log.lines.len(), MAX_LINES);
    }

    #[test]
    fn invalid_log_ids_are_not_truncated_to_another_log() {
        let mut viewer = Viewer::new(1);
        let mut items = records();
        items[2]["log"]["id"] = json!(4_294_967_297_i64);
        viewer.update(Value::Null, items);
        assert!(viewer.log_key.is_none());
    }
}
