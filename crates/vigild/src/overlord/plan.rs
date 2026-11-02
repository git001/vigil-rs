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
        let label = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let content =
            std::fs::read_to_string(&path).with_context(|| format!("reading {path:?}"))?;
        let mut layer: Layer =
            serde_yaml::from_str(&content).with_context(|| format!("parsing {path:?}"))?;
        layer.order = order as u32;
        layer.label = label;
        layers.push(layer);
    }

    let plan = Plan::from_layers(layers);
    validate(&plan)?;
    Ok(plan)
}

/// Checks that need more than deserialization. Runs on startup and on every
/// replan, so a bad layer is rejected before anything is (re)started.
fn validate(plan: &Plan) -> anyhow::Result<()> {
    use crate::process_util::EnvPolicy;

    for (name, svc) in &plan.services {
        EnvPolicy::for_service(svc).with_context(|| format!("service {name:?}"))?;
    }
    for (name, check) in &plan.checks {
        if let Some(exec) = &check.exec {
            let ctx = exec
                .service_context
                .as_deref()
                .and_then(|s| plan.services.get(s));
            EnvPolicy::for_exec_check(exec, ctx).with_context(|| format!("check {name:?}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, content: &str) {
        fs::write(dir.path().join(name), content).unwrap();
    }

    #[test]
    fn invalid_pass_env_regex_rejects_plan() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001.yaml",
            "services:\n  app:\n    command: /bin/true\n    pass-env-regex: ['(']\n",
        );
        let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
        assert!(err.contains("service \"app\""), "{err}");
        assert!(err.contains("invalid pass-env-regex"), "{err}");
    }

    #[test]
    fn pass_env_true_with_globs_rejects_plan() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001.yaml",
            "services:\n  app:\n    command: /bin/true\n    pass-env: true\n    pass-env-glob: [APP_*]\n",
        );
        assert!(load_plan(dir.path()).is_err());
    }

    #[test]
    fn exec_check_inherits_context_lists_for_validation() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001.yaml",
            "services:\n  app:\n    command: /bin/true\n    pass-env-glob: [APP_*]\n\
             checks:\n  c:\n    exec:\n      command: 'true'\n      service-context: app\n      pass-env: true\n",
        );
        let err = format!("{:#}", load_plan(dir.path()).unwrap_err());
        assert!(err.contains("check \"c\""), "{err}");
    }

    #[test]
    fn pass_env_fields_parse_and_merge() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001.yaml",
            "services:\n  app:\n    command: /bin/true\n    pass-env-glob: [APP_*]\n",
        );
        write(
            &dir,
            "002.yaml",
            "services:\n  app:\n    override: merge\n    pass-env-glob: [DB_*]\n    pass-env-regex: ['^K8S_.*']\n",
        );
        let plan = load_plan(dir.path()).unwrap();
        let app = &plan.services["app"];
        assert_eq!(app.pass_env_glob, vec!["APP_*", "DB_*"]);
        assert_eq!(app.pass_env_regex, vec!["^K8S_.*"]);
        assert_eq!(app.pass_env, None);
    }

    #[test]
    fn nonexistent_dir_returns_empty_plan() {
        let plan = load_plan(Path::new("/tmp/vigil-test-nonexistent-99999")).unwrap();
        assert!(plan.services.is_empty());
    }

    #[test]
    fn empty_dir_returns_empty_plan() {
        let dir = TempDir::new().unwrap();
        let plan = load_plan(dir.path()).unwrap();
        assert!(plan.services.is_empty());
    }

    #[test]
    fn single_valid_layer_parsed() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001-app.yaml",
            "services:\n  myapp:\n    command: /bin/echo hello\n",
        );
        let plan = load_plan(dir.path()).unwrap();
        assert!(plan.services.contains_key("myapp"));
        assert_eq!(
            plan.services["myapp"].command.as_deref().unwrap(),
            "/bin/echo hello"
        );
    }

    #[test]
    fn layers_sorted_by_filename() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "002-b.yaml",
            "services:\n  svc:\n    command: /bin/b\n",
        );
        write(
            &dir,
            "001-a.yaml",
            "services:\n  svc:\n    command: /bin/a\n",
        );
        // 002 should override 001 because sorting is by filename
        let plan = load_plan(dir.path()).unwrap();
        assert_eq!(plan.services["svc"].command.as_deref().unwrap(), "/bin/b");
    }

    #[test]
    fn non_yaml_files_are_ignored() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001-app.yaml",
            "services:\n  myapp:\n    command: /bin/echo\n",
        );
        write(&dir, "README.md", "# not yaml");
        write(&dir, "config.json", "{}");
        let plan = load_plan(dir.path()).unwrap();
        assert_eq!(plan.services.len(), 1);
        assert!(plan.services.contains_key("myapp"));
    }

    #[test]
    fn yml_extension_is_accepted() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001-app.yml",
            "services:\n  myapp:\n    command: /bin/echo\n",
        );
        let plan = load_plan(dir.path()).unwrap();
        assert!(plan.services.contains_key("myapp"));
    }

    #[test]
    fn invalid_yaml_returns_error() {
        let dir = TempDir::new().unwrap();
        write(&dir, "001-bad.yaml", "services: [\ninvalid yaml {{{\n");
        let err = load_plan(dir.path());
        assert!(err.is_err());
        let msg = format!("{}", err.unwrap_err());
        assert!(msg.contains("001-bad.yaml") || msg.contains("parsing"));
    }

    #[test]
    fn multiple_services_across_layers() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001-base.yaml",
            "services:\n  alpha:\n    command: /bin/alpha\n  beta:\n    command: /bin/beta\n",
        );
        write(
            &dir,
            "002-extra.yaml",
            "services:\n  gamma:\n    command: /bin/gamma\n",
        );
        let plan = load_plan(dir.path()).unwrap();
        assert!(plan.services.contains_key("alpha"));
        assert!(plan.services.contains_key("beta"));
        assert!(plan.services.contains_key("gamma"));
    }

    #[test]
    fn layer_order_field_set_by_position() {
        // Test that layers have increasing order values based on filename sort
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "001-first.yaml",
            "services:\n  svc:\n    command: /bin/first\n",
        );
        write(
            &dir,
            "002-second.yaml",
            "services:\n  svc:\n    command: /bin/second\n",
        );
        // We can't inspect layer.order directly via Plan, but we can verify
        // that the latter layer wins (order-based merge)
        let plan = load_plan(dir.path()).unwrap();
        assert_eq!(
            plan.services["svc"].command.as_deref().unwrap(),
            "/bin/second"
        );
    }

    #[test]
    fn empty_yaml_file_returns_empty_plan() {
        let dir = TempDir::new().unwrap();
        write(&dir, "001-empty.yaml", "");
        let plan = load_plan(dir.path()).unwrap();
        assert!(plan.services.is_empty());
    }
}
