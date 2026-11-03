//! Portable, append-only causal history. No filesystem paths or credentials
//! are representable in the wire types below.
use crate::model::{Connection, Profile, normalized_name};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use uuid::Uuid;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Portable {
    pub name: String,
    pub tags: Vec<String>,
    pub connection: Target,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Target {
    Manual {
        host: String,
        user: String,
        port: u16,
    },
    SshConfig {
        source: Uuid,
        alias: String,
    },
}

impl Portable {
    pub fn from_profile(p: &Profile) -> Self {
        Self {
            name: p.name.clone(),
            tags: p.tags.clone(),
            connection: match &p.connection {
                Connection::Manual {
                    host, user, port, ..
                } => Target::Manual {
                    host: host.clone(),
                    user: user.clone(),
                    port: *port,
                },
                Connection::SshConfig { alias, .. } => Target::SshConfig {
                    source: p.import_source.expect("sources assigned before projection"),
                    alias: alias.clone(),
                },
            },
        }
    }

    fn profile(
        &self,
        id: Uuid,
        old: Option<&Profile>,
        bindings: &BTreeMap<Uuid, PathBuf>,
    ) -> Profile {
        let (connection, source) = match &self.connection {
            Target::Manual { host, user, port } => (
                Connection::Manual {
                    host: host.clone(),
                    user: user.clone(),
                    port: *port,
                    identity_file: old.and_then(|p| match &p.connection {
                        Connection::Manual { identity_file, .. } => identity_file.clone(),
                        _ => None,
                    }),
                },
                None,
            ),
            Target::SshConfig { source, alias } => (
                Connection::SshConfig {
                    path: bindings.get(source).cloned().unwrap_or_default(),
                    alias: alias.clone(),
                },
                Some(*source),
            ),
        };
        let mut p = old
            .cloned()
            .unwrap_or_else(|| Profile::new(self.name.clone(), connection.clone()));
        let same_target = old.is_some_and(|p| {
            let mut p = p.clone();
            if matches!(p.connection, Connection::SshConfig { .. }) && p.import_source.is_none() {
                p.import_source = source;
            }
            Self::from_profile(&p).connection == self.connection
        });
        p.id = id;
        p.name = self.name.clone();
        p.tags = self.tags.clone();
        p.connection = connection;
        p.import_source = source;
        if !same_target {
            p.key_login_configured = false;
        }
        p
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.name.len() <= 4096 && self.tags.len() <= 1024,
            "云端连接字段过大"
        );
        self.profile(Uuid::nil(), None, &BTreeMap::new()).validate()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub id: Uuid,
    pub record: Uuid,
    pub parents: Vec<Uuid>,
    pub value: Option<Portable>,
    /// Dismisses a conflict branch without deleting its surviving sibling.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub branch_delete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    pub version: u32,
    pub changes: Vec<Change>,
}

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub account: Option<String>,
    pub email: String,
    pub enabled: bool,
    pub events: BTreeMap<Uuid, Change>,
    pub pending: BTreeSet<Uuid>,
    pub seen_files: BTreeSet<String>,
    pub bindings: BTreeMap<Uuid, PathBuf>,
    pub baseline: BTreeMap<Uuid, Portable>,
    pub lineage: BTreeMap<Uuid, Lineage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lineage {
    pub record: Uuid,
    pub heads: Vec<Uuid>,
}

impl State {
    pub fn assign_sources(&mut self, profiles: &mut [Profile]) {
        for p in profiles {
            if let Connection::SshConfig { path, .. } = &p.connection {
                let source = p.import_source.unwrap_or_else(|| {
                    self.bindings
                        .iter()
                        .find(|(_, existing)| *existing == path)
                        .map(|(id, _)| *id)
                        .unwrap_or_else(Uuid::new_v4)
                });
                p.import_source = Some(source);
                if !path.as_os_str().is_empty() {
                    self.bindings.insert(source, path.clone());
                }
            }
        }
    }

    /// Called in the same transaction as local edits, also detects external edits.
    pub fn capture(&mut self, profiles: &mut [Profile]) {
        self.assign_sources(profiles);
        if self.account.is_none() {
            return;
        }
        let current: BTreeMap<_, _> = profiles
            .iter()
            .map(|p| (p.id, Portable::from_profile(p)))
            .collect();
        let ids: BTreeSet<_> = self
            .baseline
            .keys()
            .chain(current.keys())
            .copied()
            .collect();
        for id in ids {
            if self.baseline.get(&id) == current.get(&id) {
                continue;
            }
            let lineage = self.lineage.get(&id).cloned().unwrap_or(Lineage {
                record: id,
                heads: vec![],
            });
            let mut value = current.get(&id).cloned();
            // Display-only collision suffixes must not become part of the
            // canonical name when the user edits another field.
            if let Some(value) = &mut value
                && self
                    .baseline
                    .get(&id)
                    .is_some_and(|old| old.name == value.name)
                && let Some(original) = lineage
                    .heads
                    .iter()
                    .filter_map(|head| self.events.get(head))
                    .find_map(|event| event.value.as_ref())
            {
                value.name = original.name.clone();
            }
            let event = Change {
                id: Uuid::new_v4(),
                record: lineage.record,
                parents: lineage.heads,
                value,
                branch_delete: !current.contains_key(&id) && id != lineage.record,
            };
            self.lineage.insert(
                id,
                Lineage {
                    record: event.record,
                    heads: vec![event.id],
                },
            );
            self.pending.insert(event.id);
            self.events.insert(event.id, event);
        }
        self.baseline = current;
    }

    pub fn batch(&self) -> Batch {
        Batch {
            version: 1,
            changes: self
                .pending
                .iter()
                .filter_map(|id| self.events.get(id).cloned())
                .collect(),
        }
    }

    /// Stage every downloaded batch before validating dependencies; Drive list
    /// order is unspecified and a child may be returned before its parent.
    pub fn ingest(&mut self, batches: &[Batch]) -> Result<()> {
        for batch in batches {
            ensure!(
                batch.version == 1 && batch.changes.len() <= 100_000,
                "不支持的云端同步格式"
            );
            for e in &batch.changes {
                ensure!(
                    !e.id.is_nil() && !e.record.is_nil() && e.parents.len() <= 1024,
                    "云端变更无效"
                );
                if let Some(value) = &e.value {
                    value.validate()?;
                }
                if let Some(old) = self.events.get(&e.id) {
                    ensure!(old == e, "云端变更 ID 内容冲突");
                }
                self.events.insert(e.id, e.clone());
            }
        }
        ensure!(
            self.events.len() <= 100_000,
            "同步历史超过首版上限，请备份后联系维护者"
        );
        // Kahn traversal validates missing parents, cross-record links and cycles.
        let mut degrees = BTreeMap::new();
        let mut children: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
        for (key, e) in &self.events {
            ensure!(
                *key == e.id && (!e.branch_delete || e.value.is_none()),
                "云端变更标识无效"
            );
            let parents: BTreeSet<_> = e.parents.iter().copied().collect();
            ensure!(parents.len() == e.parents.len(), "云端版本引用重复");
            for parent in parents {
                ensure!(
                    self.events
                        .get(&parent)
                        .is_some_and(|p| p.record == e.record),
                    "云端历史不完整，请稍后重试"
                );
                children.entry(parent).or_default().push(e.id);
            }
            degrees.insert(e.id, e.parents.len());
        }
        let mut ready: Vec<_> = degrees
            .iter()
            .filter(|(_, n)| **n == 0)
            .map(|(id, _)| *id)
            .collect();
        let mut count = 0;
        while let Some(id) = ready.pop() {
            count += 1;
            for child in children.get(&id).into_iter().flatten() {
                let degree = degrees.get_mut(child).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    ready.push(*child);
                }
            }
        }
        ensure!(count == self.events.len(), "云端版本引用存在循环");
        Ok(())
    }

    pub fn materialize(&mut self, profiles: &mut Vec<Profile>) -> Result<()> {
        let superseded: BTreeSet<_> = self
            .events
            .values()
            .flat_map(|e| e.parents.iter().copied())
            .collect();
        let mut roots: BTreeMap<Uuid, Vec<&Change>> = BTreeMap::new();
        for e in self.events.values().filter(|e| !superseded.contains(&e.id)) {
            roots.entry(e.record).or_default().push(e);
        }
        let old: BTreeMap<_, _> = profiles.iter().map(|p| (p.id, p)).collect();
        let mut result = vec![];
        let mut lineage = BTreeMap::new();
        for (root, heads) in roots {
            let deleted: Vec<_> = heads
                .iter()
                .filter(|e| e.value.is_none())
                .map(|e| e.id)
                .collect();
            let mut groups: Vec<(Portable, Vec<Uuid>)> = vec![];
            for e in heads.iter().filter(|e| e.value.is_some()) {
                let value = e.value.as_ref().unwrap();
                if let Some((_, ids)) = groups.iter_mut().find(|(v, _)| v == value) {
                    ids.push(e.id);
                } else {
                    groups.push((value.clone(), vec![e.id]));
                }
            }
            let delete_conflict = heads.iter().filter(|e| e.value.is_none()).any(|deletion| {
                !deletion.branch_delete
                    || heads.iter().filter(|e| e.value.is_some()).any(|live| {
                        let mut stack = live.parents.clone();
                        let mut visited = BTreeSet::new();
                        while let Some(parent) = stack.pop() {
                            if deletion.parents.contains(&parent) {
                                return true;
                            }
                            if visited.insert(parent)
                                && let Some(event) = self.events.get(&parent)
                            {
                                stack.extend(&event.parents);
                            }
                        }
                        false
                    })
            });
            let conflict = groups.len() > 1 || (!groups.is_empty() && delete_conflict);
            // Every conflicting branch has an ID derived from its content,
            // independent of download order and duplicate equivalent edits.
            for (value, mut ids) in groups {
                let id = if conflict {
                    Uuid::new_v5(&root, &serde_json::to_vec(&value)?)
                } else {
                    root
                };
                let mut ancestors = BTreeSet::new();
                let mut stack = ids.clone();
                while let Some(ancestor) = stack.pop() {
                    if ancestors.insert(ancestor)
                        && let Some(event) = self.events.get(&ancestor)
                    {
                        stack.extend(&event.parents);
                    }
                }
                let previous = old
                    .get(&id)
                    .copied()
                    .or_else(|| {
                        profiles.iter().find(|p| {
                            self.lineage.get(&p.id).is_some_and(|l| {
                                l.record == root
                                    && l.heads.iter().any(|head| ancestors.contains(head))
                            })
                        })
                    })
                    .or_else(|| old.get(&root).copied());
                let mut p = value.profile(id, previous, &self.bindings);
                p.sync_conflict = conflict;
                if conflict {
                    p.name = format!("{}（冲突 {}）", p.name, &id.simple().to_string()[..8]);
                }
                ids.extend(&deleted);
                lineage.insert(
                    id,
                    Lineage {
                        record: root,
                        heads: ids,
                    },
                );
                result.push(p);
            }
        }
        result.sort_by_key(|p| p.id);
        let mut used = BTreeSet::new();
        let mut used_ids = BTreeSet::new();
        for p in &mut result {
            ensure!(used_ids.insert(p.id), "云端连接 ID 冲突，已停止合并");
            let base = p.name.clone();
            if !used.insert(normalized_name(&p.name)) {
                p.name = format!("{}（{}）", base, p.id.simple());
                while !used.insert(normalized_name(&p.name)) {
                    p.name.push('·');
                }
            }
            let previous = old.get(&p.id).copied();
            p.revision = previous.map_or(1, |old| {
                let mut proposed = p.clone();
                proposed.revision = old.revision;
                if &proposed == old {
                    old.revision
                } else {
                    old.revision.saturating_add(1)
                }
            });
            p.validate()?;
        }
        self.baseline = result
            .iter()
            .map(|p| (p.id, Portable::from_profile(p)))
            .collect();
        self.lineage = lineage;
        *profiles = result;
        Ok(())
    }
}
