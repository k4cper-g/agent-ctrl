//! The cross-platform action vocabulary.
//!
//! Mirrors the platform-agnostic subset of agent-browser's action dispatcher.
//! Browser-specific verbs (`tab_*`, `waitforurl`, `route`, `har_*`, cookie
//! storage) are intentionally absent - they have no native analog. New
//! cross-platform verbs that the browser doesn't need (`switch_app`,
//! `focus_window`) are added at the bottom.

use serde::{Deserialize, Serialize};

use crate::node::RefId;

/// Action a [`Surface`](crate::surface::Surface) can be asked to perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    /// Single primary-button click on the referenced element.
    Click {
        /// Target element.
        ref_id: RefId,
    },
    /// Double primary-button click.
    DoubleClick {
        /// Target element.
        ref_id: RefId,
    },
    /// Secondary-button (right) click.
    RightClick {
        /// Target element.
        ref_id: RefId,
    },
    /// Move the cursor over the element.
    Hover {
        /// Target element.
        ref_id: RefId,
    },
    /// Move keyboard focus to the element.
    Focus {
        /// Target element.
        ref_id: RefId,
    },
    /// Type a string at the current focus.
    Type {
        /// Text to type.
        text: String,
    },
    /// Replace the value of an editable element.
    Fill {
        /// Target element.
        ref_id: RefId,
        /// New value.
        value: String,
    },
    /// Press one or more keys (e.g. `"Enter"`, `"Ctrl+A"`).
    Press {
        /// Keys in platform-agnostic notation.
        keys: String,
    },
    /// Press a key down without releasing.
    KeyDown {
        /// Key name.
        key: String,
    },
    /// Release a previously-pressed key.
    KeyUp {
        /// Key name.
        key: String,
    },
    /// Scroll the referenced element by `(dx, dy)` logical pixels.
    /// When `ref_id` is `None`, scrolls the focused viewport.
    Scroll {
        /// Element to scroll within.
        #[serde(skip_serializing_if = "Option::is_none")]
        ref_id: Option<RefId>,
        /// Horizontal delta.
        dx: f64,
        /// Vertical delta.
        dy: f64,
    },
    /// Drag from one element to another.
    Drag {
        /// Source element.
        from: RefId,
        /// Destination element.
        to: RefId,
    },
    /// Choose an option in a select / combo box / list box.
    Select {
        /// Select container.
        ref_id: RefId,
        /// Value or visible text of the option to choose.
        value: String,
    },
    /// Select all content in the focused field, or in the referenced field.
    SelectAll {
        /// Field to operate on; `None` uses current focus.
        #[serde(skip_serializing_if = "Option::is_none")]
        ref_id: Option<RefId>,
    },
    /// Scroll the element into view without focusing it.
    ScrollIntoView {
        /// Target element.
        ref_id: RefId,
    },
    /// Sleep for `ms` milliseconds.
    Wait {
        /// Duration in milliseconds.
        ms: u64,
    },
    /// Bring a different application to the foreground.
    SwitchApp {
        /// Platform application id (bundle id, package name, AUMID).
        app_id: String,
    },
    /// Bring a different window of the current app to the foreground.
    FocusWindow {
        /// Platform window id.
        window_id: String,
    },
    /// Capture a screenshot, optionally cropped to a region.
    Screenshot {
        /// Optional crop region in screen coordinates.
        #[serde(skip_serializing_if = "Option::is_none")]
        region: Option<Region>,
    },
}

/// Pixel rectangle in screen-space.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Region {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
}

/// Result of executing an [`Action`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    /// Whether the action completed successfully.
    pub ok: bool,
    /// Optional human-readable status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Free-form payload (screenshot bytes, returned text, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ActionResult {
    /// Build a successful result with no payload.
    #[must_use]
    pub fn ok() -> Self {
        Self {
            ok: true,
            message: None,
            data: None,
        }
    }

    /// Build a failure result with a message.
    #[must_use]
    pub fn fail(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            message: Some(message.into()),
            data: None,
        }
    }
}
