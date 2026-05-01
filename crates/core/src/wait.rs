//! Predicates, options, and outcomes for the cross-platform `wait-for` verb.
//!
//! Implementation strategy is **polling**, not event subscription. Each
//! platform's a11y event API has a different (and tricky) threading model
//! (UIA's `IUIAutomationEventHandler` requires careful apartment handling,
//! AX uses `AXObserver` with run-loop integration); polling is the
//! reliability floor every surface clears for free.
//!
//! Three predicates are supported, each with a distinct reliability story:
//!
//! - [`WaitPredicate::Appears`] - at least one node matches the query. Has
//!   a real race window: a node can appear in the tree before its children
//!   or state are fully populated. For racy follow-up actions, chain with
//!   [`WaitPredicate::Stable`].
//! - [`WaitPredicate::Gone`] - no node matches. Trivially correct because
//!   the predicate is "absence in the cached tree."
//! - [`WaitPredicate::Stable`] - the tree's structural signature has been
//!   unchanged for `idle_ms`. The honest "let the UI settle" primitive,
//!   which dodges the question of *which* node to wait on.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::node::{Checked, Node};
use crate::snapshot::{FindMatch, FindQuery, Snapshot};

/// What to wait for. The daemon evaluates this against a fresh snapshot on
/// each poll iteration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum WaitPredicate {
    /// Wait for at least one node matching `query` to appear.
    ///
    /// Race window: native a11y trees can publish a node before its
    /// children or state are fully populated. Agents that need to act on
    /// the matched node should chain [`Self::Stable`] afterwards.
    Appears {
        /// Filters describing the node to wait for.
        query: FindQuery,
    },
    /// Wait until no node matches `query`.
    ///
    /// More reliable than [`Self::Appears`] - disappearance is a clean
    /// "absence" signal that doesn't suffer the partial-population race.
    Gone {
        /// Filters describing the node to wait for absence of.
        query: FindQuery,
    },
    /// Wait until the tree's structural signature has been unchanged for
    /// `idle_ms` consecutive milliseconds.
    Stable {
        /// Minimum quiet period that counts as "settled."
        idle_ms: u64,
    },
}

/// Knobs for a single `wait-for` invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitOptions {
    /// What to wait for.
    pub predicate: WaitPredicate,
    /// Maximum total wait, in ms. The poll loop checks this before each
    /// `tokio::time::sleep`, so the actual elapsed time can overrun by up
    /// to one snapshot duration.
    pub timeout_ms: u64,
    /// Interval between snapshot attempts, in ms. Floored at 50 by the
    /// daemon since faster polling burns CPU without helping reliability -
    /// a UIA tree walk on a heavy app already takes 100-300ms.
    pub poll_ms: u64,
}

/// Outcome of a `wait-for` invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum WaitOutcome {
    /// `Appears` predicate matched; `found` is the first matching node
    /// from the satisfying snapshot.
    Matched {
        /// First match from the satisfying snapshot. `None` only if the
        /// predicate was satisfied but the matching node had no ref (which
        /// `Snapshot::find` already filters out, so in practice `Some`).
        found: Option<FindMatch>,
        /// Wall-clock ms from the first poll to the satisfying poll.
        elapsed_ms: u64,
    },
    /// `Gone` predicate satisfied - no node matched on the most recent poll.
    Gone {
        /// Wall-clock ms from the first poll to the satisfying poll.
        elapsed_ms: u64,
    },
    /// `Stable` predicate satisfied - the tree signature held for `idle_ms`.
    Stable {
        /// Wall-clock ms from the first poll to the satisfying poll.
        elapsed_ms: u64,
    },
    /// `timeout_ms` elapsed before the predicate was satisfied.
    Timeout {
        /// Total elapsed ms (close to `timeout_ms`, give or take one poll).
        elapsed_ms: u64,
    },
}

/// Compute a content hash of the snapshot's structural state.
///
/// Intentionally excludes volatile fields so that hover highlights, focus
/// transitions, layout jitter, and native-handle churn don't reset the
/// `Stable` predicate's quiet timer:
///
/// - **Excluded:** bounds, focused, description, level, native handle, opaque.
/// - **Included:** ref_id presence, role, name, value, enabled, checked,
///   expanded, selected, child count.
///
/// The included set is the minimum that captures "the user-visible state
/// of every interactive control" - exactly the set an agent cares about
/// when waiting for the UI to settle.
#[must_use]
pub fn tree_signature(snap: &Snapshot) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_node(&snap.root, &mut hasher);
    snap.refs.len().hash(&mut hasher);
    hasher.finish()
}

fn hash_node<H: Hasher>(node: &Node, h: &mut H) {
    match &node.ref_id {
        Some(r) => {
            1_u8.hash(h);
            r.0.hash(h);
        }
        None => 0_u8.hash(h),
    }
    node.role.hash(h);
    node.name.hash(h);
    node.value.as_deref().hash(h);
    node.state.enabled.hash(h);
    encode_checked(node.state.checked).hash(h);
    encode_tristate(node.state.expanded).hash(h);
    encode_tristate(node.state.selected).hash(h);
    node.children.len().hash(h);
    for child in &node.children {
        hash_node(child, h);
    }
}

const fn encode_checked(c: Option<Checked>) -> u8 {
    match c {
        None => 0,
        Some(Checked::False) => 1,
        Some(Checked::True) => 2,
        Some(Checked::Mixed) => 3,
    }
}

const fn encode_tristate(t: Option<bool>) -> u8 {
    match t {
        None => 0,
        Some(false) => 1,
        Some(true) => 2,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::time::SystemTime;

    use super::*;
    use crate::node::{AppContext, Bounds, RefId, State};
    use crate::role::Role;
    use crate::snapshot::RefMap;
    use crate::surface::SurfaceKind;

    fn make_snapshot(button_name: &str, bounds: Bounds) -> Snapshot {
        let mut refs = RefMap::new();
        let id = refs.insert(Role::Button, button_name.into(), 0, None);
        let button = Node {
            ref_id: Some(id),
            role: Role::Button,
            name: button_name.into(),
            description: None,
            value: None,
            state: State {
                visible: true,
                enabled: true,
                ..State::default()
            },
            bounds: Some(bounds),
            level: None,
            children: Vec::new(),
            opaque: false,
            native: None,
        };
        let root = Node {
            ref_id: None,
            role: Role::Window,
            name: "Window".into(),
            description: None,
            value: None,
            state: State::default(),
            bounds: None,
            level: None,
            children: vec![button],
            opaque: false,
            native: None,
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
    fn signature_is_stable_across_identical_snapshots() {
        let a = make_snapshot(
            "OK",
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 30.0,
            },
        );
        let b = make_snapshot(
            "OK",
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 30.0,
            },
        );
        assert_eq!(tree_signature(&a), tree_signature(&b));
    }

    #[test]
    fn signature_ignores_bounds_jitter() {
        let a = make_snapshot(
            "OK",
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 30.0,
            },
        );
        let b = make_snapshot(
            "OK",
            Bounds {
                x: 1.0,
                y: 0.0,
                w: 80.0,
                h: 30.0,
            },
        );
        assert_eq!(
            tree_signature(&a),
            tree_signature(&b),
            "bounds jitter should not invalidate stability"
        );
    }

    #[test]
    fn signature_changes_when_name_changes() {
        let a = make_snapshot(
            "OK",
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 30.0,
            },
        );
        let b = make_snapshot(
            "Cancel",
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 30.0,
            },
        );
        assert_ne!(tree_signature(&a), tree_signature(&b));
    }

    #[test]
    fn signature_changes_when_child_count_changes() {
        let mut a = make_snapshot(
            "OK",
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 30.0,
            },
        );
        let mut b = make_snapshot(
            "OK",
            Bounds {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 30.0,
            },
        );
        // Mutate b so it has an extra leaf child under the root.
        b.root.children.push(Node {
            ref_id: Some(RefId::new(99)),
            role: Role::Button,
            name: "Extra".into(),
            description: None,
            value: None,
            state: State::default(),
            bounds: None,
            level: None,
            children: Vec::new(),
            opaque: false,
            native: None,
        });
        // a stays unchanged for contrast.
        let _ = a.refs.insert(Role::Button, "Untouched".into(), 0, None);
        assert_ne!(tree_signature(&a), tree_signature(&b));
    }

    #[test]
    fn wait_options_round_trip_json() {
        let opts = WaitOptions {
            predicate: WaitPredicate::Stable { idle_ms: 500 },
            timeout_ms: 10_000,
            poll_ms: 250,
        };
        let json = serde_json::to_string(&opts).unwrap();
        assert!(json.contains(r#""kind":"stable""#));
        let back: WaitOptions = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            back.predicate,
            WaitPredicate::Stable { idle_ms: 500 }
        ));
    }
}
