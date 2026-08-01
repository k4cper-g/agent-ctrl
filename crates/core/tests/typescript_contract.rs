//! Guard the hand-written TypeScript wire unions against Rust enum drift.

#![allow(clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

fn source(path: &str) -> String {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join(path)).expect("contract source should be readable")
}

fn match_labels<'a>(source: &'a str, marker: &str) -> Vec<&'a str> {
    let body = source
        .split_once(marker)
        .expect("contract function marker should exist")
        .1
        .split_once("\n    }\n}")
        .expect("contract function should have a closing brace")
        .0;
    body.lines()
        .filter_map(|line| line.split_once("=> \"").map(|(_, value)| value))
        .filter_map(|value| value.split_once('"').map(|(label, _)| label))
        .collect()
}

fn section<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    source
        .split_once(start)
        .expect("TypeScript section start should exist")
        .1
        .split_once(end)
        .expect("TypeScript section end should exist")
        .0
}

fn quoted_values(source: &str) -> BTreeSet<&str> {
    source.split('"').skip(1).step_by(2).collect()
}

#[test]
fn typescript_surface_and_action_labels_exactly_match_rust() {
    let surface_source = source("src/surface.rs");
    let action_source = source("src/action.rs");
    let typescript = source("../../packages/client/src/types.ts");

    let surface_labels = match_labels(&surface_source, "pub fn as_str(self)")
        .into_iter()
        .collect::<BTreeSet<_>>();
    let action_labels = match_labels(&action_source, "pub const fn kind_name(&self)")
        .into_iter()
        .collect::<BTreeSet<_>>();
    assert!(!surface_labels.is_empty());
    assert!(!action_labels.is_empty());

    let typescript_surfaces =
        quoted_values(section(&typescript, "export type SurfaceKind = ", ";"));
    let typescript_actions = quoted_values(section(
        &typescript,
        "export type Action =",
        "export type ClipboardOp",
    ));

    assert_eq!(typescript_surfaces, surface_labels);
    assert_eq!(typescript_actions, action_labels);
}

#[test]
fn typescript_protocol_version_matches_daemon() {
    let daemon = source("../daemon/src/dispatcher.rs");
    let typescript = source("../../packages/client/src/types.ts");
    let version = daemon
        .split_once("pub const PROTOCOL_VERSION: u32 = ")
        .and_then(|(_, rest)| rest.split_once(';'))
        .map(|(value, _)| value.trim())
        .expect("daemon protocol version constant should exist");
    assert!(
        typescript.contains(&format!("export const PROTOCOL_VERSION = {version};")),
        "TypeScript protocol version differs from daemon version {version}"
    );
}
