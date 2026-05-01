//! # agent-ctrl-core
//!
//! Shared types and the [`Surface`](surface::Surface) trait every platform implements.
//!
//! This crate has no platform-specific dependencies. It defines:
//!
//! - The unified element schema ([`node`])
//! - The cross-platform action vocabulary ([`action`])
//! - The surface contract ([`surface`])
//! - The ARIA-derived role taxonomy ([`role`])
//! - The single-snapshot [`RefMap`](snapshot::RefMap) ([`snapshot`])
//! - The error type ([`error`])
//!
//! ## Design rules
//!
//! - **Single-snapshot stable refs.** A [`RefId`](node::RefId) is only valid for
//!   the snapshot that produced it. Surfaces re-resolve refs to real native
//!   elements at action time.
//! - **Capability negotiation.** Surfaces advertise a [`CapabilitySet`](surface::CapabilitySet);
//!   callers must check `supports(...)` before invoking optional actions.
//! - **No platform leakage.** Anything OS-specific is hidden behind
//!   [`NativeHandle`](node::NativeHandle).

#![forbid(unsafe_code)]

pub mod action;
pub mod error;
pub mod node;
pub mod role;
pub mod snapshot;
pub mod surface;
pub mod wait;

#[cfg(feature = "mock")]
pub mod mock;

pub use action::{Action, ActionResult, Region};
pub use error::{Error, Result};
pub use node::{AppContext, Bounds, Checked, NativeHandle, Node, RefId, State, WindowContext};
pub use role::Role;
pub use snapshot::{
    FindMatch, FindQuery, RefEntry, RefMap, Snapshot, SnapshotOptions, WindowTarget,
};
pub use surface::{CapabilitySet, Surface, SurfaceKind, WindowInfo};
pub use wait::{tree_signature, WaitOptions, WaitOutcome, WaitPredicate};

#[cfg(feature = "mock")]
pub use mock::MockSurface;
