//! The unified element node and its supporting types.
//!
//! Modeled on agent-browser's internal `TreeNode`, with the browser-specific
//! `backend_node_id` replaced by a [`NativeHandle`] enum and `app`/`window`
//! context lifted to the [`Snapshot`](crate::snapshot::Snapshot) level.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::role::Role;

/// Stable identifier for a node within a single snapshot.
///
/// The id is opaque to the agent and only valid for the snapshot that
/// produced it. After a fresh snapshot, refs must be re-issued.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RefId(pub String);

impl RefId {
    /// Build the canonical ref for the `n`-th interactive element in a snapshot.
    #[must_use]
    pub fn new(n: usize) -> Self {
        Self(format!("ref_{n}"))
    }
}

impl fmt::Display for RefId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Logical-pixel screen-space rectangle.
///
/// Coordinates are DPI-normalized so the same value means the same physical
/// position regardless of the OS scale factor.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

/// Per-platform identifier for the underlying native element.
///
/// Never exposed to the agent. Surfaces use it as a fast-path hint when
/// re-resolving a [`RefId`] at action time; if the handle has been invalidated
/// (a common occurrence in native a11y trees) the surface falls back to the
/// `role + name + nth` triple stored in the [`RefMap`](crate::snapshot::RefMap).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "kebab-case")]
pub enum NativeHandle {
    /// Chrome DevTools Protocol backend node id.
    Cdp {
        /// CDP `BackendNodeId`.
        backend_node_id: i64,
    },
    /// Windows UI Automation handle.
    Uia {
        /// UIA `RuntimeId` — unstable across runs but useful within one session.
        runtime_id: Vec<u8>,
        /// UIA `AutomationId` when set; the most stable identifier UIA exposes.
        automation_id: Option<String>,
    },
    /// macOS Accessibility handle.
    Ax {
        /// Process-local pointer to the `AXUIElement`.
        element_ref: u64,
    },
    /// Android AccessibilityNodeInfo handle.
    Android {
        /// Window id assigned by the Android AccessibilityService.
        window_id: i32,
        /// Virtual view id within the window.
        virtual_view_id: i64,
        /// `android:id` resource name when present.
        resource_id: Option<String>,
    },
    /// iOS XCUIElement reference.
    Ios {
        /// XCUITest element identifier.
        element_id: String,
    },
}

/// Tristate state for checkboxes, radios, and similar controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Checked {
    /// Checked / on.
    True,
    /// Unchecked / off.
    False,
    /// Indeterminate / mixed (e.g. parent of partially-checked children).
    Mixed,
}

/// Element state flags exposed by every accessibility platform.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// Whether the element is currently displayed and not occluded.
    pub visible: bool,
    /// Whether the element accepts user input.
    pub enabled: bool,
    /// Whether the element currently has keyboard focus.
    pub focused: bool,
    /// Selection state for selectable items (rows, options, tabs).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    /// Check state for checkboxes / radios.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked: Option<Checked>,
    /// Expansion state for collapsibles (tree items, disclosures).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expanded: Option<bool>,
    /// Whether the field is marked required by the application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
}

/// One element in the unified accessibility tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Node {
    /// Stable per-snapshot identifier; only assigned to nodes the agent can target.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<RefId>,

    /// Canonical role.
    pub role: Role,

    /// Primary accessible label (always a string, may be empty).
    pub name: String,

    /// Longer description when distinct from `name`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Current value for editable / value-bearing elements.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,

    /// State flags.
    pub state: State,

    /// Screen-space bounds when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<Bounds>,

    /// Heading level (1-6) for `Role::Heading`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i32>,

    /// Child nodes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Node>,

    /// Set when the platform exposed nothing useful for this subtree.
    /// A future vision/OCR fallback will target opaque regions.
    #[serde(default, skip_serializing_if = "is_false")]
    pub opaque: bool,

    /// Platform-specific handle, used internally by the surface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeHandle>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// Application context shared by every node in a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppContext {
    /// Bundle id (macOS), package name (Android), AUMID (Windows), origin (web).
    pub id: String,
    /// Human-readable application name.
    pub name: String,
}

/// Window or tab context for a snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowContext {
    /// Platform-assigned window id.
    pub id: String,
    /// Window title bar text or browser tab title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}
