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
    /// Set a checkable control to the checked state.
    Check {
        /// Target element.
        ref_id: RefId,
    },
    /// Set a checkable control to the unchecked state.
    Uncheck {
        /// Target element.
        ref_id: RefId,
    },
    /// Toggle a checkable control.
    Toggle {
        /// Target element.
        ref_id: RefId,
    },
    /// Clear an editable field.
    Clear {
        /// Target element.
        ref_id: RefId,
    },
    /// Read from, write to, copy from, or paste from the host clipboard.
    Clipboard {
        /// Clipboard operation to perform.
        op: ClipboardOp,
    },
    /// Raw mouse event in screen coordinates.
    Mouse {
        /// Mouse operation to perform.
        op: MouseOp,
    },
    /// Visually mark an element for human debugging.
    Highlight {
        /// Target element.
        ref_id: RefId,
        /// Highlight duration in milliseconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
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
        /// Optional target. When absent, `region` preserves the legacy shape.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<ScreenshotTarget>,
        /// Draw visible ref labels on the screenshot when supported.
        #[serde(default, skip_serializing_if = "is_false")]
        annotated: bool,
    },
}

/// Clipboard operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ClipboardOp {
    /// Read Unicode text from the clipboard.
    Read,
    /// Replace clipboard text.
    Write {
        /// Text to store.
        text: String,
    },
    /// Send the platform copy shortcut.
    Copy,
    /// Send the platform paste shortcut.
    Paste,
}

/// Raw mouse operation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MouseOp {
    /// Move the cursor to screen coordinates.
    Move {
        /// X coordinate.
        x: i32,
        /// Y coordinate.
        y: i32,
    },
    /// Press a mouse button at screen coordinates.
    Down {
        /// X coordinate.
        x: i32,
        /// Y coordinate.
        y: i32,
        /// Button to press.
        button: MouseButton,
    },
    /// Release a mouse button at screen coordinates.
    Up {
        /// X coordinate.
        x: i32,
        /// Y coordinate.
        y: i32,
        /// Button to release.
        button: MouseButton,
    },
    /// Send a wheel event at screen coordinates.
    Wheel {
        /// X coordinate.
        x: i32,
        /// Y coordinate.
        y: i32,
        /// Horizontal wheel delta.
        dx: i32,
        /// Vertical wheel delta.
        dy: i32,
    },
}

/// Mouse button name.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// Primary button.
    Left,
    /// Secondary button.
    Right,
    /// Middle button.
    Middle,
}

/// Screenshot target.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScreenshotTarget {
    /// Capture the current pinned window.
    Window,
    /// Capture the desktop or virtual screen.
    Desktop,
    /// Capture a specific region.
    Region {
        /// Region to capture.
        region: Region,
    },
    /// Capture the bounds of an element ref.
    Ref {
        /// Element whose bounds define the screenshot region.
        ref_id: RefId,
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

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
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
