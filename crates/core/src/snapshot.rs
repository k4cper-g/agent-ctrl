//! Snapshot output and the [`RefMap`] keystone.

use std::collections::HashMap;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::node::{AppContext, NativeHandle, Node, RefId, WindowContext};
use crate::role::Role;
use crate::surface::SurfaceKind;

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
    /// without first looking up its PID — but title text is locale-dependent
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

/// Bidirectional map from agent-facing [`RefId`]s to the data needed to
/// rediscover the underlying native element at action time.
///
/// Native a11y handles are routinely invalidated by the OS (window relayouts,
/// tree mutations, focus changes), so we never expose them to the agent.
/// Instead the agent uses a [`RefId`] and surfaces re-walk the tree using the
/// `(role, name, nth)` triple — durable across most tree mutations — plus an
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
    use super::*;

    #[test]
    fn refmap_assigns_sequential_ids() {
        let mut map = RefMap::new();
        let a = map.insert(Role::Button, "OK".into(), 0, None);
        let b = map.insert(Role::Button, "Cancel".into(), 0, None);
        assert_eq!(a.0, "ref_0");
        assert_eq!(b.0, "ref_1");
        assert_eq!(map.len(), 2);
    }
}
