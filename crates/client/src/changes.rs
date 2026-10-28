//! Committed Model invalidation candidates, separate from SQL table notifications.
use axton_core::RecordKey;
use std::collections::BTreeMap;

// A slow consumer cannot retain an unbounded identity journal. Once a Model
// crosses this bound, its next refresh reads the committed result in full.
const MAX_KEYS_PER_MODEL: usize = 4096;

#[derive(Clone, Default)]
pub(crate) struct ModelChanges {
    pub(crate) models: BTreeMap<String, ModelChange>,
}

#[derive(Clone)]
pub(crate) enum ModelChange {
    Keys(BTreeMap<String, RecordKey>),
    All,
}

impl ModelChanges {
    pub(crate) fn all(&mut self, model: &str) {
        self.models.insert(model.to_string(), ModelChange::All);
    }

    pub(crate) fn record(&mut self, encoded: String, key: RecordKey) {
        let change = self
            .models
            .entry(key.model.clone())
            .or_insert_with(|| ModelChange::Keys(BTreeMap::new()));
        if let ModelChange::Keys(keys) = change {
            keys.insert(encoded, key);
            if keys.len() > MAX_KEYS_PER_MODEL {
                *change = ModelChange::All;
            }
        }
    }

    pub(crate) fn merge(&mut self, other: Self) {
        for (model, change) in other.models {
            match change {
                ModelChange::All => self.all(&model),
                ModelChange::Keys(keys) => {
                    for (encoded, key) in keys {
                        self.record(encoded, key);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accumulating_commits_deduplicates_keys_and_bounds_the_journal() {
        let mut changes = ModelChanges::default();
        for i in 0..=MAX_KEYS_PER_MODEL {
            let key = RecordKey {
                model: "Entry".into(),
                identity: json!({"id": i}),
            };
            let mut commit = ModelChanges::default();
            commit.record(key.encoded().unwrap(), key.clone());
            changes.merge(commit.clone());
            changes.merge(commit);
            if i < MAX_KEYS_PER_MODEL {
                let ModelChange::Keys(keys) = &changes.models["Entry"] else {
                    panic!("early full refresh")
                };
                assert_eq!(keys.len(), i + 1);
            }
        }
        assert!(matches!(changes.models["Entry"], ModelChange::All));
    }
}
