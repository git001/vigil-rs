// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Templates in a service's `command` and `environment` values.
//!
//! The syntax is minijinja (Jinja2), the same as `body-template` of alerts:
//!
//! ```text
//! {{ env.PORT | default("8080") }}      variable with fallback
//! {{ file("/etc/secrets/db") }}         file content (read on every start)
//! {{ mem("90% - 64MiB") }}              memory budget in MiB
//! {{ cpu("100% - 1") }}                 CPU count
//! ```
//!
//! Only values containing `{{` or `{%` are rendered, so plain values (and
//! shell constructs like `${#VAR}`) are left alone. Rendering happens in one
//! pass: a substituted value is never parsed as a template again. Undefined
//! variables are errors (use `default` or `is defined`). Rendered values are
//! never logged.

pub mod env_file;
mod file;
mod resources;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::hash::Hasher;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use minijinja::{Environment, UndefinedBehavior, context};

/// Whether `s` would be rendered at all.
pub fn is_template(s: &str) -> bool {
    s.contains("{{") || s.contains("{%")
}

/// Longest time vigild waits for file reads (templates, env files, watched
/// files). A read on a dead NFS hard mount never returns and cannot be
/// cancelled; with this limit it only ties up one blocking-pool thread, never
/// the async runtime or the service's actor.
pub const FILE_IO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run `f` (which may read files) on the blocking pool, giving up after
/// [`FILE_IO_TIMEOUT`].
pub async fn off_runtime<T: Send + 'static>(
    what: &str,
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    off_runtime_within(FILE_IO_TIMEOUT, what, f).await
}

async fn off_runtime_within<T: Send + 'static>(
    limit: std::time::Duration,
    what: &str,
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    match tokio::time::timeout(limit, tokio::task::spawn_blocking(f)).await {
        Ok(Ok(res)) => res,
        Ok(Err(e)) => Err(anyhow::anyhow!("{what}: {e}")),
        Err(_) => Err(anyhow::anyhow!(
            "{what}: reading files did not finish within {:?} (hanging file system, e.g. a dead NFS mount?)",
            limit
        )),
    }
}

/// A file read while building a service's start: its path and a hash of
/// the content that was used (`None`: it did not exist). `watch-files`
/// compares against exactly this state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: PathBuf,
    pub hash: Option<u64>,
}

/// Hash used to detect content changes (not cryptographic; only compared
/// with earlier hashes of the same file within one vigild process).
#[derive(Default)]
pub struct ContentHasher(std::collections::hash_map::DefaultHasher);

impl ContentHasher {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    pub fn finish(&self) -> u64 {
        self.0.finish()
    }
}

pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut h = ContentHasher::default();
    h.update(bytes);
    h.finish()
}

/// Collects the files a [`Renderer`] reads.
#[derive(Clone, Default)]
pub(crate) struct Reads(Arc<Mutex<Vec<FileRead>>>);

impl Reads {
    pub(crate) fn record(&self, path: &str, content: Option<&str>) {
        let entry = FileRead {
            path: PathBuf::from(path),
            hash: content.map(|c| content_hash(c.as_bytes())),
        };
        let mut v = self.0.lock().expect("reads lock");
        if !v.iter().any(|r| r.path == entry.path) {
            v.push(entry);
        }
    }

    fn take(&self) -> Vec<FileRead> {
        std::mem::take(&mut *self.0.lock().expect("reads lock"))
    }
}

fn environment(reads: Reads) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_keep_trailing_newline(true);
    env.add_function("file", move |path: String, default: Option<String>| {
        file::file(&reads, path, default)
    });
    env.add_function("mem", resources::mem);
    env.add_function("cpu", resources::cpu);
    env
}

/// Check the syntax of a service's templates (at load and replan).
/// Values are not resolved.
pub fn check_service(cfg: &vigil_types::plan::ServiceConfig) -> anyhow::Result<()> {
    let env = environment(Reads::default());
    if let Some(cmd) = &cfg.command {
        for word in crate::process_util::parse_command_templated(cmd)? {
            if is_template(&word) {
                env.template_from_str(&word)
                    .map_err(|e| anyhow::anyhow!("template in command word {word:?}: {e}"))?;
            }
        }
    }
    check_environment(&env, &cfg.environment)?;
    for entry in &cfg.env_file {
        env_file::check_entry(entry)?;
    }
    Ok(())
}

/// Check the syntax of an exec check's `environment` templates.
pub fn check_exec_environment(values: &IndexMap<String, String>) -> anyhow::Result<()> {
    check_environment(&environment(Reads::default()), values)
}

fn check_environment(
    env: &Environment<'static>,
    values: &IndexMap<String, String>,
) -> anyhow::Result<()> {
    for (k, v) in values {
        if is_template(v) {
            env.template_from_str(v)
                .map_err(|e| anyhow::anyhow!("template in environment {k:?}: {e}"))?;
        }
    }
    Ok(())
}

/// Explicit variables of a service (or exec check): `env-file` contents,
/// then the rendered `environment` values.
pub struct EnvLayer<'a> {
    pub env_file: &'a [String],
    pub environment: &'a IndexMap<String, String>,
}

/// Build a child's environment: what `policy` lets through from vigild's
/// environment, then each layer's env files and `environment` values.
///
/// Templates in a layer's `environment` see everything before them (base
/// set, pass-env, env files, earlier layers), but not the other entries of
/// the same `environment`. vigild's whole environment is never visible.
pub fn child_env(
    renderer: &Renderer,
    policy: &crate::process_util::EnvPolicy,
    creds: Option<&crate::process_util::Credentials>,
    layers: &[EnvLayer<'_>],
) -> anyhow::Result<Vec<(OsString, OsString)>> {
    // Read once so every build uses the same snapshot.
    let parent: Vec<_> = std::env::vars_os().collect();
    let mut explicit: IndexMap<String, String> = IndexMap::new();
    for layer in layers {
        env_file::load(layer.env_file, &mut explicit, &renderer.reads)?;
        let vars = vars_of(&policy.build(parent.clone(), creds, &explicit));
        for (k, v) in layer.environment {
            let v = renderer
                .render(v, &vars)
                .map_err(|e| anyhow::anyhow!("environment {k:?}: {e}"))?;
            explicit.insert(k.clone(), v);
        }
    }
    Ok(policy.build(parent, creds, &explicit))
}

/// Environment entries with UTF-8 names and values, as template variables.
pub fn vars_of(env: &[(OsString, OsString)]) -> BTreeMap<String, String> {
    env.iter()
        .filter_map(|(k, v)| Some((k.to_str()?.to_owned(), v.to_str()?.to_owned())))
        .collect()
}

/// Renders templates for one service start and records the files read.
pub struct Renderer {
    env: Environment<'static>,
    reads: Reads,
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

impl Renderer {
    pub fn new() -> Self {
        let reads = Reads::default();
        Self {
            env: environment(reads.clone()),
            reads,
        }
    }

    /// Files read so far by `file()` and `env-file` (each path once, first
    /// read wins), in the order they were read.
    pub fn take_reads(&self) -> Vec<FileRead> {
        self.reads.take()
    }

    /// Render `s` with `vars` available as `env`. Plain strings are returned
    /// unchanged.
    /// Errors carry minijinja's message (never the rendered value).
    pub fn render(&self, s: &str, vars: &BTreeMap<String, String>) -> anyhow::Result<String> {
        if !is_template(s) {
            return Ok(s.to_owned());
        }
        let out = self
            .env
            .template_from_str(s)
            .and_then(|t| t.render(context! { env => vars }))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        anyhow::ensure!(!out.contains('\0'), "rendered value contains a NUL byte");
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
