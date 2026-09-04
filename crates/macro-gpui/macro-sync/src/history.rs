use anyhow::{Context as _, Result};
use loro::{ChangeMeta, Frontiers, ID, LoroDoc, ToJson};
use std::{collections::HashMap, ops::ControlFlow};

pub const HISTORY_SESSION_GAP_MS: i64 = 10 * 60 * 1_000;
pub const BOGUS_BLANK_MARKDOWN_HISTORY_MS: i64 = 1_780_516_229_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryVersionId {
    pub peer: u64,
    pub counter: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistorySession {
    pub user_id: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub count: usize,
}

/// A full-history Loro snapshot indexed with the same causal-frontier
/// algorithm as Macro's `timestampIndex.ts`.
pub struct DocumentHistory {
    document: LoroDoc,
    changes: Vec<ChangeMeta>,
}

impl DocumentHistory {
    pub fn from_snapshot(snapshot: &[u8]) -> Result<Self> {
        let document = LoroDoc::new();
        document
            .import(snapshot)
            .context("failed to import document history snapshot")?;
        let frontier_ids = document.oplog_frontiers().iter().collect::<Vec<_>>();
        let mut changes = Vec::with_capacity(document.len_changes());
        document
            .travel_change_ancestors(&frontier_ids, &mut |change| {
                changes.push(change);
                ControlFlow::Continue(())
            })
            .context("failed to traverse document change history")?;
        changes.sort_by(|left, right| {
            left.timestamp
                .cmp(&right.timestamp)
                .then(left.id.counter.cmp(&right.id.counter))
                .then(left.id.peer.cmp(&right.id.peer))
        });
        Ok(Self { document, changes })
    }

    pub fn sessions(&self, users_by_peer: &HashMap<String, String>) -> Vec<HistorySession> {
        let events = self.changes.iter().map(|change| {
            let peer = change.id.peer.to_string();
            let user_id = users_by_peer
                .get(&peer)
                .cloned()
                .unwrap_or_else(|| format!("peer:{peer}"));
            (user_id, change.timestamp.saturating_mul(1_000))
        });
        sessionize(events, HISTORY_SESSION_GAP_MS)
    }

    pub fn preview_at(&self, timestamp_ms: i64) -> Result<Option<serde_json::Value>> {
        let Some(frontiers) = self.frontiers_at(timestamp_ms) else {
            return Ok(None);
        };
        let historic = self
            .document
            .fork_at(&frontiers)
            .context("failed to check out historic document frontier")?;
        Ok(Some(historic.get_deep_value().to_json_value()))
    }

    pub fn version_at(&self, timestamp_ms: i64) -> Option<HistoryVersionId> {
        let frontiers = self.frontiers_at(timestamp_ms)?;
        frontiers.iter().last().map(|id| HistoryVersionId {
            peer: id.peer,
            counter: id.counter,
        })
    }

    pub fn change_count(&self) -> usize {
        self.changes.len()
    }

    fn frontiers_at(&self, timestamp_ms: i64) -> Option<Frontiers> {
        let mut frontier = HashMap::<u64, i32>::new();
        for change in &self.changes {
            if change.timestamp.saturating_mul(1_000) > timestamp_ms {
                break;
            }
            let end_counter = change
                .id
                .counter
                .saturating_add(change.len.saturating_sub(1) as i32);
            for dependency in change.deps.iter() {
                if frontier
                    .get(&dependency.peer)
                    .is_some_and(|counter| *counter <= dependency.counter)
                {
                    frontier.remove(&dependency.peer);
                }
            }
            frontier
                .entry(change.id.peer)
                .and_modify(|counter| *counter = (*counter).max(end_counter))
                .or_insert(end_counter);
        }
        if frontier.is_empty() {
            None
        } else {
            Some(Frontiers::from(
                frontier
                    .into_iter()
                    .map(|(peer, counter)| ID::new(peer, counter))
                    .collect::<Vec<_>>(),
            ))
        }
    }
}

pub fn sessionize(
    events: impl IntoIterator<Item = (String, i64)>,
    gap_ms: i64,
) -> Vec<HistorySession> {
    let mut by_user = HashMap::<String, Vec<i64>>::new();
    for (user_id, timestamp_ms) in events {
        by_user.entry(user_id).or_default().push(timestamp_ms);
    }

    let mut sessions = Vec::new();
    for (user_id, mut timestamps) in by_user {
        timestamps.sort_unstable();
        let Some(mut start_ms) = timestamps.first().copied() else {
            continue;
        };
        let mut end_ms = start_ms;
        let mut count = 1usize;
        for timestamp_ms in timestamps.into_iter().skip(1) {
            if timestamp_ms.saturating_sub(end_ms) > gap_ms {
                sessions.push(HistorySession {
                    user_id: user_id.clone(),
                    start_ms,
                    end_ms,
                    count,
                });
                start_ms = timestamp_ms;
                count = 0;
            }
            end_ms = timestamp_ms;
            count += 1;
        }
        sessions.push(HistorySession {
            user_id,
            start_ms,
            end_ms,
            count,
        });
    }
    sessions.sort_by(|left, right| {
        right
            .end_ms
            .cmp(&left.end_ms)
            .then(right.start_ms.cmp(&left.start_ms))
            .then(left.user_id.cmp(&right.user_id))
    });
    sessions.retain(|session| {
        !(session.start_ms == BOGUS_BLANK_MARKDOWN_HISTORY_MS
            && session.end_ms == BOGUS_BLANK_MARKDOWN_HISTORY_MS
            && session.count == 1)
    });
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_match_macro_gap_and_sorting_contract() {
        let sessions = sessionize(
            [
                ("alice".to_owned(), 1_000),
                ("alice".to_owned(), 2_000),
                ("alice".to_owned(), HISTORY_SESSION_GAP_MS + 3_000),
                ("bob".to_owned(), 4_000),
            ],
            HISTORY_SESSION_GAP_MS,
        );
        assert_eq!(sessions.len(), 3);
        assert_eq!(sessions[0].start_ms, HISTORY_SESSION_GAP_MS + 3_000);
        assert_eq!(sessions[1].user_id, "bob");
        assert_eq!(sessions[2].count, 2);
    }

    #[test]
    fn full_snapshot_supports_sessions_and_historic_checkout() {
        let document = LoroDoc::new();
        document.set_record_timestamp(true);
        document.set_change_merge_interval(0);
        document.set_peer_id(7).unwrap();
        document.set_next_commit_timestamp(10);
        document.get_map("root").insert("value", "first").unwrap();
        document.commit();
        document.set_next_commit_timestamp(20);
        document.get_map("root").insert("value", "second").unwrap();
        document.commit();
        let snapshot = document
            .export(loro::ExportMode::Snapshot)
            .expect("snapshot export");

        let history = DocumentHistory::from_snapshot(&snapshot).unwrap();
        assert_eq!(history.change_count(), 2);
        let sessions = history.sessions(&HashMap::from([("7".to_owned(), "alice".to_owned())]));
        assert_eq!(sessions[0].count, 2);
        let preview = history.preview_at(15_000).unwrap().unwrap();
        assert_eq!(preview["root"]["value"], "first");
        assert_eq!(history.version_at(15_000).unwrap().peer, 7);
    }
}
