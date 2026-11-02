// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Environment of child processes (services and exec checks).
//!
//! By default a child does **not** inherit vigild's environment, which may
//! hold secrets (e.g. Kubernetes `env`/`envFrom` from Secrets). It gets a
//! minimal base (`PATH`, `HOME`, `USER`, `LOGNAME`, `TZ`), the vigild
//! variables selected by `pass-env-glob` / `pass-env-regex`, and its own
//! `environment` on top. `pass-env: true` restores full inheritance.

use std::ffi::OsString;

use anyhow::Context;
use indexmap::IndexMap;
use regex::Regex;
use tracing::debug;

use vigil_types::plan::{ExecCheck, ServiceConfig};

use super::Credentials;

/// Used when vigild itself has no `PATH`.
pub const DEFAULT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Which of vigild's environment variables a child receives.
#[derive(Debug)]
pub struct EnvPolicy {
    inherit_all: bool,
    globs: Vec<String>,
    regexes: Vec<Regex>,
}

impl EnvPolicy {
    /// Compile the policy. Errors on invalid regular expressions and on
    /// `pass-env: true` combined with glob/regex lists (they would have no
    /// effect, which is almost certainly a mistake).
    pub fn compile(
        pass_env: Option<bool>,
        globs: &[String],
        regexes: &[String],
    ) -> anyhow::Result<Self> {
        let inherit_all = pass_env.unwrap_or(false);
        anyhow::ensure!(
            !(inherit_all && (!globs.is_empty() || !regexes.is_empty())),
            "`pass-env: true` passes the whole environment; \
             `pass-env-glob` / `pass-env-regex` would have no effect"
        );
        let regexes = regexes
            .iter()
            .map(|r| {
                Regex::new(&format!("^(?:{r})$"))
                    .with_context(|| format!("invalid pass-env-regex {r:?}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(EnvPolicy {
            inherit_all,
            globs: globs.to_vec(),
            regexes,
        })
    }

    /// Whether vigild's variable `name` is passed through by glob/regex.
    fn selects(&self, name: &str) -> bool {
        self.globs.iter().any(|g| glob_match(g, name))
            || self.regexes.iter().any(|r| r.is_match(name))
    }

    /// Build the complete child environment.
    ///
    /// - `parent`: vigild's environment (`std::env::vars_os()` in production).
    /// - `creds`: the identity the child switches to, if any. Then `HOME`,
    ///   `USER` and `LOGNAME` describe the target user instead of vigild's.
    /// - `explicit`: the configured `environment`, applied last.
    pub fn build(
        &self,
        parent: impl IntoIterator<Item = (OsString, OsString)>,
        creds: Option<&Credentials>,
        explicit: &IndexMap<String, String>,
    ) -> Vec<(OsString, OsString)> {
        let parent: Vec<(OsString, OsString)> = parent.into_iter().collect();
        let mut env: IndexMap<OsString, OsString> = IndexMap::new();

        if self.inherit_all {
            env.extend(parent);
        } else {
            let get = |k: &str| parent.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());

            env.insert(
                "PATH".into(),
                get("PATH").unwrap_or_else(|| DEFAULT_PATH.into()),
            );
            if let Some(tz) = get("TZ") {
                env.insert("TZ".into(), tz);
            }

            let target = creds.and_then(|c| nix::unistd::User::from_uid(c.uid).ok().flatten());
            match target {
                Some(pw) => {
                    env.insert("HOME".into(), pw.dir.into_os_string());
                    env.insert("USER".into(), pw.name.clone().into());
                    env.insert("LOGNAME".into(), pw.name.into());
                }
                None => {
                    for k in ["HOME", "USER", "LOGNAME"] {
                        if let Some(v) = get(k) {
                            env.insert(k.into(), v);
                        }
                    }
                }
            }

            let mut passed = Vec::new();
            for (name, value) in &parent {
                // Names that are not UTF-8 cannot be matched; never passed.
                if let Some(n) = name.to_str()
                    && self.selects(n)
                {
                    env.insert(name.clone(), value.clone());
                    passed.push(n.to_owned());
                }
            }
            if !passed.is_empty() {
                debug!(vars = ?passed, "passing vigild environment variables");
            }
        }

        for (k, v) in explicit {
            env.insert(k.into(), v.into());
        }
        env.into_iter().collect()
    }
}

impl EnvPolicy {
    /// Policy of a service.
    pub fn for_service(cfg: &ServiceConfig) -> anyhow::Result<Self> {
        Self::compile(cfg.pass_env, &cfg.pass_env_glob, &cfg.pass_env_regex)
    }

    /// Policy of an exec check: the check's `pass-env` overrides the service
    /// context's, glob/regex lists are combined (service first).
    pub fn for_exec_check(exec: &ExecCheck, ctx: Option<&ServiceConfig>) -> anyhow::Result<Self> {
        let pass_env = exec.pass_env.or_else(|| ctx.and_then(|s| s.pass_env));
        let mut globs: Vec<String> = ctx.map(|s| s.pass_env_glob.clone()).unwrap_or_default();
        globs.extend(exec.pass_env_glob.iter().cloned());
        let mut regexes: Vec<String> = ctx.map(|s| s.pass_env_regex.clone()).unwrap_or_default();
        regexes.extend(exec.pass_env_regex.iter().cloned());
        Self::compile(pass_env, &globs, &regexes)
    }
}

/// Apply a built environment to `cmd`, replacing everything inherited.
pub fn apply_env(cmd: &mut tokio::process::Command, env: Vec<(OsString, OsString)>) {
    cmd.env_clear();
    cmd.envs(env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())));
}

/// Whole-string wildcard match: `*` matches any run of characters
/// (including none), `?` exactly one character. Case-sensitive.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    // Iterative matcher with backtracking to the last `*`.
    let (mut pi, mut ni) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some((pi, ni));
            pi += 1;
        } else if let Some((sp, sn)) = star {
            pi = sp + 1;
            ni = sn + 1;
            star = Some((sp, sn + 1));
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn os(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsStr::new(k).to_owned(), OsStr::new(v).to_owned()))
            .collect()
    }

    fn names(env: &[(OsString, OsString)]) -> Vec<String> {
        env.iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect()
    }

    fn value<'a>(env: &'a [(OsString, OsString)], k: &str) -> Option<&'a OsStr> {
        env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_os_str())
    }

    fn parent() -> Vec<(OsString, OsString)> {
        os(&[
            ("PATH", "/bin"),
            ("HOME", "/root"),
            ("USER", "root"),
            ("TZ", "Europe/Vienna"),
            ("DB_PASSWORD", "s3cret"),
            ("DATABASE_URL", "postgres://x"),
            ("APP_MODE", "prod"),
            ("X_APP_FOO_LAL", "1"),
            ("K8S_API_URL", "https://k8s"),
            ("MY_TOKEN_SECRET", "t"),
            ("TOKEN_A", "a"),
        ])
    }

    #[test]
    fn glob_rules() {
        assert!(glob_match("APP_*", "APP_MODE"));
        assert!(glob_match("APP_*", "APP_"));
        assert!(!glob_match("APP_*", "MY_APP_MODE"));
        assert!(glob_match("*_APP_*_LAL", "X_APP_FOO_LAL"));
        assert!(glob_match("*_APP_*_LAL", "A_B_APP_C_D_LAL"));
        assert!(!glob_match("*_APP_*_LAL", "X_APP_FOO_LALA"));
        assert!(glob_match("DATABASE_URL", "DATABASE_URL"));
        assert!(!glob_match("DATABASE_URL", "DATABASE_URL2"));
        assert!(glob_match("TOKEN_?", "TOKEN_A"));
        assert!(!glob_match("TOKEN_?", "TOKEN_AB"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("", "X"));
        assert!(glob_match("**A**", "BAB"));
    }

    #[test]
    fn default_passes_only_base_vars() {
        let p = EnvPolicy::compile(None, &[], &[]).unwrap();
        let env = p.build(parent(), None, &IndexMap::new());
        assert_eq!(names(&env), vec!["PATH", "TZ", "HOME", "USER"]);
        assert!(
            value(&env, "DB_PASSWORD").is_none(),
            "secrets must not leak"
        );
    }

    #[test]
    fn default_path_when_vigild_has_none() {
        let p = EnvPolicy::compile(Some(false), &[], &[]).unwrap();
        let env = p.build(os(&[("HOME", "/root")]), None, &IndexMap::new());
        assert_eq!(value(&env, "PATH"), Some(OsStr::new(DEFAULT_PATH)));
    }

    #[test]
    fn inherit_all_passes_everything() {
        let p = EnvPolicy::compile(Some(true), &[], &[]).unwrap();
        let env = p.build(parent(), None, &IndexMap::new());
        assert_eq!(env.len(), parent().len());
        assert_eq!(value(&env, "DB_PASSWORD"), Some(OsStr::new("s3cret")));
    }

    #[test]
    fn inherit_all_with_lists_is_rejected() {
        let err = EnvPolicy::compile(Some(true), &["APP_*".into()], &[]).unwrap_err();
        assert!(err.to_string().contains("no effect"), "{err}");
        assert!(EnvPolicy::compile(Some(true), &[], &["X".into()]).is_err());
    }

    #[test]
    fn globs_select_whole_names() {
        let globs = vec!["DATABASE_URL".into(), "APP_*".into(), "*_APP_*_LAL".into()];
        let p = EnvPolicy::compile(None, &globs, &[]).unwrap();
        let env = p.build(parent(), None, &IndexMap::new());
        for k in ["DATABASE_URL", "APP_MODE", "X_APP_FOO_LAL"] {
            assert!(value(&env, k).is_some(), "{k} missing");
        }
        assert!(value(&env, "DB_PASSWORD").is_none());
    }

    #[test]
    fn regexes_are_anchored() {
        let res = vec!["^K8S_[A-Z]+_URL$".into(), "TOKEN_(A|B)".into()];
        let p = EnvPolicy::compile(None, &[], &res).unwrap();
        let env = p.build(parent(), None, &IndexMap::new());
        assert!(value(&env, "K8S_API_URL").is_some());
        assert!(value(&env, "TOKEN_A").is_some());
        assert!(
            value(&env, "MY_TOKEN_SECRET").is_none(),
            "unanchored match must not happen"
        );
    }

    #[test]
    fn partial_regex_needs_explicit_wildcards() {
        let p = EnvPolicy::compile(None, &[], &[".*TOKEN.*".into()]).unwrap();
        let env = p.build(parent(), None, &IndexMap::new());
        assert!(value(&env, "MY_TOKEN_SECRET").is_some());
    }

    #[test]
    fn invalid_regex_is_an_error() {
        let err = EnvPolicy::compile(None, &[], &["(".into()]).unwrap_err();
        assert!(err.to_string().contains("invalid pass-env-regex"), "{err}");
    }

    #[test]
    fn explicit_environment_wins() {
        let p = EnvPolicy::compile(None, &["APP_*".into()], &[]).unwrap();
        let mut explicit = IndexMap::new();
        explicit.insert("APP_MODE".to_string(), "dev".to_string());
        explicit.insert("PATH".to_string(), "/opt/bin".to_string());
        let env = p.build(parent(), None, &explicit);
        assert_eq!(value(&env, "APP_MODE"), Some(OsStr::new("dev")));
        assert_eq!(value(&env, "PATH"), Some(OsStr::new("/opt/bin")));
    }

    #[test]
    fn switched_user_gets_its_own_home_and_name() {
        let me = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
            .unwrap()
            .expect("current user has a passwd entry");
        let creds = Credentials {
            uid: me.uid,
            gid: me.gid,
            groups: vec![me.gid],
        };
        let p = EnvPolicy::compile(None, &[], &[]).unwrap();
        let env = p.build(parent(), Some(&creds), &IndexMap::new());
        assert_eq!(value(&env, "HOME"), Some(me.dir.as_os_str()));
        assert_eq!(value(&env, "USER"), Some(OsStr::new(&me.name)));
        assert_eq!(value(&env, "LOGNAME"), Some(OsStr::new(&me.name)));
    }
}
