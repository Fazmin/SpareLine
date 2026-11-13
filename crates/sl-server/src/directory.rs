//! Group isolation: an in-memory snapshot of who belongs to which Group, rebuilt from
//! the database whenever membership changes. Every visibility decision goes through here.
//!
//! Rules:
//! - A user can see another user only if they share at least one Group.
//! - A direct chat is allowed only between users who share a Group (read-only otherwise).
//! - A group chat is scoped to one Group; every member must belong to it.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use parking_lot::RwLock;
use rusqlite::Connection;
use sl_proto::{GroupId, GroupRef, UserId};

use crate::error::AppResult;

#[derive(Debug, Clone)]
pub struct UserBasic {
    pub username: String,
    pub display_name: String,
    pub disabled: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Visibility {
    pub user_groups: HashMap<UserId, BTreeSet<GroupId>>,
    pub group_users: HashMap<GroupId, BTreeSet<UserId>>,
    pub group_names: HashMap<GroupId, String>,
    pub users: HashMap<UserId, UserBasic>,
}

impl Visibility {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        let mut v = Visibility::default();
        let mut st = conn.prepare("SELECT id, username, display_name, disabled FROM users")?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, UserId>(0)?,
                UserBasic {
                    username: r.get(1)?,
                    display_name: r.get(2)?,
                    disabled: r.get::<_, i64>(3)? != 0,
                },
            ))
        })?;
        for row in rows {
            let (id, u) = row?;
            v.users.insert(id, u);
        }
        let mut st = conn.prepare("SELECT id, name FROM groups")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, GroupId>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (id, name) = row?;
            v.group_names.insert(id, name);
            v.group_users.entry(id).or_default();
        }
        let mut st = conn.prepare("SELECT group_id, user_id FROM group_members")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, GroupId>(0)?, r.get::<_, UserId>(1)?))
        })?;
        for row in rows {
            let (g, u) = row?;
            v.group_users.entry(g).or_default().insert(u);
            v.user_groups.entry(u).or_default().insert(g);
        }
        Ok(v)
    }

    pub fn groups_of(&self, user: &UserId) -> BTreeSet<GroupId> {
        self.user_groups.get(user).cloned().unwrap_or_default()
    }

    pub fn group_refs_of(&self, user: &UserId) -> Vec<GroupRef> {
        let mut out: Vec<GroupRef> = self
            .groups_of(user)
            .into_iter()
            .map(|id| GroupRef {
                id,
                name: self.group_names.get(&id).cloned().unwrap_or_default(),
            })
            .collect();
        out.sort_by_key(|a| a.name.to_lowercase());
        out
    }

    pub fn shared_groups(&self, a: &UserId, b: &UserId) -> BTreeSet<GroupId> {
        match (self.user_groups.get(a), self.user_groups.get(b)) {
            (Some(ga), Some(gb)) => ga.intersection(gb).copied().collect(),
            _ => BTreeSet::new(),
        }
    }

    /// Can `viewer` see `other`? (Yourself always; disabled users never.)
    pub fn can_see(&self, viewer: &UserId, other: &UserId) -> bool {
        if viewer == other {
            return true;
        }
        if self.users.get(other).is_none_or(|u| u.disabled) {
            return false;
        }
        !self.shared_groups(viewer, other).is_empty()
    }

    pub fn is_member(&self, user: &UserId, group: &GroupId) -> bool {
        self.group_users
            .get(group)
            .is_some_and(|s| s.contains(user))
    }

    pub fn is_active(&self, user: &UserId) -> bool {
        self.users.get(user).is_some_and(|u| !u.disabled)
    }

    /// Everyone `viewer` can see (excluding themselves).
    pub fn visible_users(&self, viewer: &UserId) -> BTreeSet<UserId> {
        let mut out = BTreeSet::new();
        for g in self.groups_of(viewer) {
            if let Some(us) = self.group_users.get(&g) {
                for u in us {
                    if u != viewer && self.is_active(u) {
                        out.insert(*u);
                    }
                }
            }
        }
        out
    }
}

/// Shared, swappable snapshot.
#[derive(Default)]
pub struct DirectoryCache {
    snap: RwLock<Arc<Visibility>>,
}

impl DirectoryCache {
    pub fn get(&self) -> Arc<Visibility> {
        self.snap.read().clone()
    }
    pub fn set(&self, v: Visibility) {
        *self.snap.write() = Arc::new(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn fixture() -> (Visibility, [UserId; 4], [GroupId; 2]) {
        let users = [
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
            Uuid::now_v7(),
        ];
        let groups = [Uuid::now_v7(), Uuid::now_v7()];
        let mut v = Visibility::default();
        for (i, u) in users.iter().enumerate() {
            v.users.insert(
                *u,
                UserBasic {
                    username: format!("u{i}"),
                    display_name: format!("U{i}"),
                    disabled: i == 3,
                },
            );
        }
        // g0: u0, u1, u3(disabled) ; g1: u1, u2
        for (g, members) in [
            (groups[0], vec![users[0], users[1], users[3]]),
            (groups[1], vec![users[1], users[2]]),
        ] {
            for m in members {
                v.group_users.entry(g).or_default().insert(m);
                v.user_groups.entry(m).or_default().insert(g);
            }
        }
        (v, users, groups)
    }

    #[test]
    fn isolation_rules() {
        let (v, u, g) = fixture();
        assert!(v.can_see(&u[0], &u[1]));
        assert!(!v.can_see(&u[0], &u[2]), "u0 and u2 share no group");
        assert!(v.can_see(&u[1], &u[2]));
        assert!(!v.can_see(&u[0], &u[3]), "disabled users are hidden");
        assert_eq!(v.visible_users(&u[0]).len(), 1);
        assert_eq!(v.visible_users(&u[1]).len(), 2);
        assert!(v.is_member(&u[2], &g[1]));
        assert!(!v.is_member(&u[2], &g[0]));
    }
}
