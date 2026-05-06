//! Cached-snapshot inspection types.

use serde::{Deserialize, Serialize};

use crate::node::RefId;

/// Field returned by a `get` request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GetField {
    /// User-visible text, using value first and name as a fallback.
    Text,
    /// Editable or value-bearing node value.
    Value,
    /// Accessible name.
    Name,
    /// Canonical role.
    Role,
    /// Full state object.
    State,
    /// Bounds in logical screen coordinates.
    Bounds,
    /// Current snapshot window context.
    Window,
}

/// Boolean state returned by an `is` request.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateField {
    /// Whether the node is visible.
    Visible,
    /// Whether the node accepts input.
    Enabled,
    /// Whether the node has keyboard focus.
    Focused,
    /// Whether the node is selected.
    Selected,
    /// Whether the node is checked.
    Checked,
    /// Whether the node is expanded.
    Expanded,
}

/// Result of a `get` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetResult {
    /// Field that was read.
    pub field: GetField,
    /// Ref that was read, absent for window-level fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_id: Option<RefId>,
    /// JSON value for the field.
    pub value: serde_json::Value,
}

/// Result of an `is` request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IsResult {
    /// State that was checked.
    pub field: StateField,
    /// Ref that was checked.
    pub ref_id: RefId,
    /// Boolean state value.
    pub value: bool,
}
