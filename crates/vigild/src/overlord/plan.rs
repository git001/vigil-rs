// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use std::path::Path;

use anyhow::Context;
use vigil_types::plan::{Layer, Plan};

pub fn load_plan(dir: &Path) -> anyhow::Result<Plan> {
    if !dir.exists() {
        return Ok(Plan::default());
    }

    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("reading layers dir {dir:?}"))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.ends_with(".yaml") || n.ends_with(".yml")
        })
        .collect();

    entries.sort_by_key(|e| e.file_name());

    let mut layers = Vec::new();
    for (order, entry) in entries.iter().enumerate() {
        let path = entry.path();
        let label = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let content =
            std::fs::read_to_string(&path).with_context(|| format!("reading {path:?}"))?;
        let mut layer: Layer =
            serde_yaml::from_str(&content).with_context(|| format!("parsing {path:?}"))?;
        layer.order = order as u32;
        layer.label = label;
        layers.push(layer);
    }

    Ok(Plan::from_layers(layers))
}
