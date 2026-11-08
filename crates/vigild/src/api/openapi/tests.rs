// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! `docs/specs/openapi.yaml` is generated from [`ApiDoc`], so the file in
//! the repository cannot drift from what vigild serves at `/openapi.json`.

use std::path::PathBuf;

use utoipa::OpenApi;

use super::ApiDoc;

const HEADER: &str = "\
# Generated from vigild's ApiDoc (crates/vigild/src/api/openapi.rs); the
# running daemon serves the same spec at /openapi.json. Do not edit by hand:
#   UPDATE_OPENAPI=1 cargo test -p vigild openapi_yaml
";

#[test]
fn openapi_yaml_is_up_to_date() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/specs/openapi.yaml");
    let want = format!(
        "{HEADER}{}",
        serde_yaml::to_string(&ApiDoc::openapi()).unwrap()
    );
    if std::env::var_os("UPDATE_OPENAPI").is_some() {
        std::fs::write(&path, &want).unwrap();
        return;
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "docs/specs/openapi.yaml is out of date; regenerate it with\n  \
         UPDATE_OPENAPI=1 cargo test -p vigild openapi_yaml"
    );
}
