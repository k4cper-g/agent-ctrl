//! Snapshot output and the [`RefMap`] keystone.

use std::collections::HashMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::node::{AppContext, NativeHandle, Node, RefId, WindowContext};
use crate::role::Role;
use crate::surface::SurfaceKind;

/// Query used by [`Snapshot::find`] to locate refs without re-snapshotting.
///
/// All fields are filters; an unset filter matches anything. Multiple filters
/// AND together. Matching always requires the node to carry a [`RefId`] -
/// non-interactive structural nodes are not returned because they cannot be
/// acted on.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FindQuery {
    /// Match against `node.name`. Case-insensitive substring by default;
    /// becomes case-sensitive equality when [`Self::exact`] is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    /// When `true`, [`Self::name`] must equal `node.name` exactly.
    #[serde(default, skip_serializing_if = "is_false")]
    pub exact: bool,

    /// Restrict matches to a single role.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,

    /// Restrict the search to the subtree rooted at this ref. The root node
    /// itself is included in the search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_ref: Option<RefId>,

    /// Cap on the number of matches returned. `None` means unlimited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// One row of [`Snapshot::find`] output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindMatch {
    /// Ref the agent uses to target this node.
    pub ref_id: RefId,
    /// Role at the time the snapshot was taken.
    pub role: Role,
    /// Name at the time the snapshot was taken.
    pub name: String,
}

/// Knobs controlling what a snapshot captures.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SnapshotOptions {
    /// CSS / platform selector to scope the snapshot to a subtree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,

    /// Emit only interactive nodes plus their structural ancestors.
    #[serde(default)]
    pub interactive: bool,

    /// Drop redundant intermediate nodes (single-child generic groups, etc.).
    #[serde(default)]
    pub compact: bool,

    /// Maximum tree depth to walk.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<usize>,

    /// Which window or process to capture. Defaults to the foreground window.
    /// Subsequent actions on this session will reuse the same window, so an
    /// agent driving a non-foreground app can pin its target up front.
    #[serde(default)]
    pub target: WindowTarget,
}

/// How to pick the window a snapshot (and subsequent actions) should bind to.
///
/// Surfaces translate this into a platform handle (HWND on Windows,
/// `AXUIElement` on macOS, etc.) at snapshot time, then keep the handle
/// pinned for actions that follow.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "kebab-case")]
pub enum WindowTarget {
    /// Whatever window currently has user focus on the host. Default.
    #[default]
    Foreground,
    /// First top-level visible window owned by this OS process.
    Pid {
        /// Process id (Windows DWORD / Unix pid).
        pid: u32,
    },
    /// First top-level visible window whose title contains the given
    /// substring (case-insensitive). Useful for pinning to a known app
    /// without first looking up its PID - but title text is locale-dependent
    /// (`"Untitled - Notepad"` in English vs `"Bez tytułu - Notatnik"` in
    /// Polish), so prefer [`Self::ProcessName`] for portable tests.
    Title {
        /// Substring to match against the window's title.
        title: String,
    },
    /// First top-level visible window owned by a process whose executable
    /// file stem matches `name` (case-insensitive, e.g. `"Notepad"` matches
    /// `notepad.exe`). Locale-independent, so this is the right choice for
    /// portable tests and most agent code.
    ProcessName {
        /// Executable file stem to match (without `.exe` extension on Windows).
        name: String,
    },
}

/// A captured accessibility snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    /// Wall-clock time the snapshot was captured.
    pub captured_at: SystemTime,

    /// Surface that produced this snapshot.
    pub surface_kind: SurfaceKind,

    /// Application context (foreground app, current tab origin, etc.).
    pub app: AppContext,

    /// Window or tab context, when meaningful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowContext>,

    /// Root of the captured tree.
    pub root: Node,

    /// Lookup table from agent-facing [`RefId`] to platform hints.
    pub refs: RefMap,
}

impl Snapshot {
    /// Search this snapshot's tree for nodes matching `query`.
    ///
    /// Returns matches in tree order (pre-order DFS). Only nodes that carry a
    /// [`RefId`] are returned, since the agent needs a ref to act on them.
    /// When `query.in_ref` is set but no node in the tree carries that ref,
    /// the result is empty.
    #[must_use]
    pub fn find(&self, query: &FindQuery) -> Vec<FindMatch> {
        let mut out = Vec::new();
        let limit = query.limit.unwrap_or(usize::MAX);
        let root = if let Some(target) = &query.in_ref {
            match find_subtree(&self.root, target) {
                Some(node) => node,
                None => return out,
            }
        } else {
            &self.root
        };
        collect_matches(root, query, limit, &mut out);
        out
    }
}

fn find_subtree<'a>(node: &'a Node, target: &RefId) -> Option<&'a Node> {
    if node.ref_id.as_ref() == Some(target) {
        return Some(node);
    }
    for child in &node.children {
        if let Some(hit) = find_subtree(child, target) {
            return Some(hit);
        }
    }
    None
}

fn collect_matches(node: &Node, query: &FindQuery, limit: usize, out: &mut Vec<FindMatch>) {
    if out.len() >= limit {
        return;
    }
    if let Some(ref_id) = &node.ref_id {
        if matches_filters(node, query) {
            out.push(FindMatch {
                ref_id: ref_id.clone(),
                role: node.role.clone(),
                name: node.name.clone(),
            });
        }
    }
    for child in &node.children {
        if out.len() >= limit {
            return;
        }
        collect_matches(child, query, limit, out);
    }
}

fn matches_filters(node: &Node, query: &FindQuery) -> bool {
    if let Some(role) = &query.role {
        if &node.role != role {
            return false;
        }
    }
    if let Some(needle) = &query.name {
        if query.exact {
            if node.name != *needle {
                return false;
            }
        } else if !node.name.to_lowercase().contains(&needle.to_lowercase()) {
            return false;
        }
    }
    true
}

/// Bidirectional map from agent-facing [`RefId`]s to the data needed to
/// rediscover the underlying native element at action time.
///
/// Native a11y handles are routinely invalidated by the OS (window relayouts,
/// tree mutations, focus changes), so we never expose them to the agent.
/// Instead the agent uses a [`RefId`] and surfaces re-walk the tree using the
/// `(role, name, nth)` triple - durable across most tree mutations - plus an
/// optional [`NativeHandle`] used as a fast-path hint.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RefMap {
    entries: HashMap<RefId, RefEntry>,
    next: usize,
}

/// One row in the [`RefMap`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RefEntry {
    /// Element role at capture time.
    pub role: Role,
    /// Element name at capture time.
    pub name: String,
    /// 0-based index disambiguating multiple nodes with the same `(role, name)`.
    pub nth: usize,
    /// Platform handle when the surface can provide one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeHandle>,
}

impl RefMap {
    /// Construct an empty `RefMap`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a fresh [`RefId`] and store its lookup entry.
    pub fn insert(
        &mut self,
        role: Role,
        name: String,
        nth: usize,
        native: Option<NativeHandle>,
    ) -> RefId {
        let id = RefId::new(self.next);
        self.next += 1;
        self.entries.insert(
            id.clone(),
            RefEntry {
                role,
                name,
                nth,
                native,
            },
        );
        id
    }

    /// Look up the entry for a [`RefId`].
    #[must_use]
    pub fn get(&self, id: &RefId) -> Option<&RefEntry> {
        self.entries.get(id)
    }

    /// Number of refs allocated.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the map has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over all `(RefId, RefEntry)` pairs.
    pub fn iter(&self) -> impl Iterator<Item = (&RefId, &RefEntry)> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::time::SystemTime;

    use super::*;
    use crate::node::{AppContext, Node, State};

    #[test]
    fn refmap_assigns_sequential_ids() {
        let mut map = RefMap::new();
        let a = map.insert(Role::Button, "OK".into(), 0, None);
        let b = map.insert(Role::Button, "Cancel".into(), 0, None);
        assert_eq!(a.0, "ref_0");
        assert_eq!(b.0, "ref_1");
        assert_eq!(map.len(), 2);
    }

    /// Build a small fixture tree:
    ///
    /// ```text
    /// window "Editor"
    ///   menubar
    ///     @e0 menuitem "File"
    ///     @e1 menuitem "Edit"
    ///   document
    ///     @e2 button "Save"
    ///     @e3 button "Save As"
    ///     @e4 button "save-lower"
    /// ```
    fn fixture() -> Snapshot {
        fn leaf(role: Role, name: &str, ref_id: Option<RefId>) -> Node {
            Node {
                ref_id,
                role,
                name: name.into(),
                description: None,
                value: None,
                state: State::default(),
                bounds: None,
                level: None,
                children: Vec::new(),
                opaque: false,
                native: None,
            }
        }

        let mut refs = RefMap::new();
        let file_id = refs.insert(Role::MenuItem, "File".into(), 0, None);
        let edit_id = refs.insert(Role::MenuItem, "Edit".into(), 0, None);
        let save_id = refs.insert(Role::Button, "Save".into(), 0, None);
        let save_as_id = refs.insert(Role::Button, "Save As".into(), 0, None);
        let lower_id = refs.insert(Role::Button, "save-lower".into(), 0, None);

        let menubar = Node {
            children: vec![
                leaf(Role::MenuItem, "File", Some(file_id)),
                leaf(Role::MenuItem, "Edit", Some(edit_id)),
            ],
            ..leaf(Role::MenuBar, "", None)
        };
        let document = Node {
            children: vec![
                leaf(Role::Button, "Save", Some(save_id)),
                leaf(Role::Button, "Save As", Some(save_as_id)),
                leaf(Role::Button, "save-lower", Some(lower_id)),
            ],
            ..leaf(Role::Document, "", None)
        };
        let root = Node {
            children: vec![menubar, document],
            ..leaf(Role::Window, "Editor", None)
        };

        Snapshot {
            captured_at: SystemTime::UNIX_EPOCH,
            surface_kind: SurfaceKind::Mock,
            app: AppContext {
                id: "fixture".into(),
                name: "Fixture".into(),
            },
            window: None,
            root,
            refs,
        }
    }

    #[test]
    fn find_substring_is_case_insensitive() {
        let snap = fixture();
        let q = FindQuery {
            name: Some("save".into()),
            ..FindQuery::default()
        };
        let names: Vec<_> = snap.find(&q).into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["Save", "Save As", "save-lower"]);
    }

    #[test]
    fn find_exact_is_case_sensitive() {
        let snap = fixture();
        let q = FindQuery {
            name: Some("Save".into()),
            exact: true,
            ..FindQuery::default()
        };
        let names: Vec<_> = snap.find(&q).into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["Save"]);
    }

    #[test]
    fn find_role_filter_excludes_other_roles() {
        let snap = fixture();
        let q = FindQuery {
            role: Some(Role::MenuItem),
            ..FindQuery::default()
        };
        let names: Vec<_> = snap.find(&q).into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["File", "Edit"]);
    }

    #[test]
    fn find_in_ref_restricts_to_subtree() {
        let snap = fixture();
        // The "File" ref is on a leaf; in_ref pointing at a leaf returns just
        // that leaf when no further filter excludes it.
        let file_ref = snap
            .refs
            .iter()
            .find(|(_, e)| e.name == "File")
            .map(|(id, _)| id.clone())
            .unwrap();
        let q = FindQuery {
            in_ref: Some(file_ref),
            ..FindQuery::default()
        };
        let names: Vec<_> = snap.find(&q).into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["File"]);
    }

    #[test]
    fn find_limit_caps_results() {
        let snap = fixture();
        let q = FindQuery {
            name: Some("save".into()),
            limit: Some(2),
            ..FindQuery::default()
        };
        assert_eq!(snap.find(&q).len(), 2);
    }

    #[test]
    fn find_unknown_in_ref_returns_empty() {
        let snap = fixture();
        let q = FindQuery {
            in_ref: Some(RefId::new(999)),
            ..FindQuery::default()
        };
        assert!(snap.find(&q).is_empty());
    }
}
