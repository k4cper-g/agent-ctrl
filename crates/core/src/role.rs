//! ARIA-derived role taxonomy.
//!
//! Mirrors the role lists in agent-browser's `snapshot.rs`
//! (`INTERACTIVE_ROLES`, `CONTENT_ROLES`, `STRUCTURAL_ROLES`) and extends
//! them with cross-platform concepts (apps, windows, frames) that desktop
//! and mobile a11y trees expose.
//!
//! Each surface implementation is responsible for mapping its native role
//! vocabulary (UIA `ControlType`, AX `AXRole`, Android class names,
//! `UIAccessibilityTraits`) into this enum.

use serde::{Deserialize, Serialize};

/// Canonical cross-platform element role.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Role {
    // -- Interactive --
    /// A pressable button.
    Button,
    /// A hyperlink.
    Link,
    /// A single- or multi-line text input.
    TextField,
    /// A checkbox (binary or tristate).
    Checkbox,
    /// A radio button.
    Radio,
    /// A combo box / dropdown with text input.
    ComboBox,
    /// A list of selectable options.
    ListBox,
    /// An item inside a menu.
    MenuItem,
    /// A checkable menu item.
    MenuItemCheckbox,
    /// A radio menu item.
    MenuItemRadio,
    /// An option inside a list box or combo box.
    Option,
    /// A search input.
    SearchBox,
    /// A continuous-range slider.
    Slider,
    /// A discrete-range numeric spinner.
    SpinButton,
    /// A binary on/off switch.
    Switch,
    /// A tab in a tab list.
    Tab,
    /// An item inside a tree.
    TreeItem,

    // -- Content --
    /// Heading at any level.
    Heading,
    /// Cell in a table or grid.
    Cell,
    /// Cell in an interactive grid.
    GridCell,
    /// Column header cell.
    ColumnHeader,
    /// Row header cell.
    RowHeader,
    /// Item inside a list.
    ListItem,
    /// Article block.
    Article,
    /// Generic landmark region.
    Region,
    /// Main landmark.
    Main,
    /// Navigation landmark.
    Navigation,

    // -- Structural --
    /// Generic container with no specific semantic.
    Generic,
    /// Generic group of related elements.
    Group,
    /// Ordered or unordered list.
    List,
    /// Table.
    Table,
    /// Row inside a table or grid.
    Row,
    /// Group of rows.
    RowGroup,
    /// Interactive grid.
    Grid,
    /// Tree-shaped grid.
    TreeGrid,
    /// Menu container.
    Menu,
    /// Persistent menu bar.
    MenuBar,
    /// Toolbar of buttons.
    Toolbar,
    /// Container of tabs.
    TabList,
    /// Tree container.
    Tree,
    /// Document landmark.
    Document,
    /// Application landmark (web) or running process (native).
    Application,
    /// OS-level window.
    Window,
    /// Modal or non-modal dialog.
    Dialog,

    // -- Container / app-level (extensions over agent-browser) --
    /// Top-level installed app on the device.
    App,
    /// Iframe (web) or embedded surface (native).
    Frame,
    /// Image element.
    Image,

    // -- Fallback --
    /// Role reported by the platform that does not map to any known variant.
    Unknown(String),
}

impl Role {
    /// Returns `true` for roles that an agent typically targets directly.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        matches!(
            self,
            Self::Button
                | Self::Link
                | Self::TextField
                | Self::Checkbox
                | Self::Radio
                | Self::ComboBox
                | Self::ListBox
                | Self::MenuItem
                | Self::MenuItemCheckbox
                | Self::MenuItemRadio
                | Self::Option
                | Self::SearchBox
                | Self::Slider
                | Self::SpinButton
                | Self::Switch
                | Self::Tab
                | Self::TreeItem
        )
    }

    /// Returns `true` for content roles that carry meaningful text.
    #[must_use]
    pub fn is_content(&self) -> bool {
        matches!(
            self,
            Self::Heading
                | Self::Cell
                | Self::GridCell
                | Self::ColumnHeader
                | Self::RowHeader
                | Self::ListItem
                | Self::Article
                | Self::Region
                | Self::Main
                | Self::Navigation
        )
    }

    /// Returns `true` for purely structural / grouping roles.
    #[must_use]
    pub fn is_structural(&self) -> bool {
        matches!(
            self,
            Self::Generic
                | Self::Group
                | Self::List
                | Self::Table
                | Self::Row
                | Self::RowGroup
                | Self::Grid
                | Self::TreeGrid
                | Self::Menu
                | Self::MenuBar
                | Self::Toolbar
                | Self::TabList
                | Self::Tree
                | Self::Document
                | Self::Application
                | Self::Window
                | Self::Dialog
                | Self::App
                | Self::Frame
        )
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn role_partitions_are_disjoint() {
        for role in [Role::Button, Role::Heading, Role::Group, Role::Image] {
            let i = u8::from(role.is_interactive());
            let c = u8::from(role.is_content());
            let s = u8::from(role.is_structural());
            assert!(i + c + s <= 1, "role {role:?} matched multiple partitions");
        }
    }

    #[test]
    fn role_serializes_kebab_case() {
        let json = serde_json::to_string(&Role::TextField).unwrap();
        assert_eq!(json, "\"text-field\"");
    }
}
