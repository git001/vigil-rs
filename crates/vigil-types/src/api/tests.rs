// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

//! Every request body is strict: an unknown field is an error, so a typo
//! cannot silently change what a request does.

use serde::de::DeserializeOwned;

use super::{CheckRefreshRequest, DaemonActionRequest, ServicesAction, SignalsRequest};
use crate::identity::{AddIdentitiesRequest, RemoveIdentitiesRequest};

/// `valid` parses; `valid` with the extra field `extra` does not, and the
/// error names that field.
fn strict<T: DeserializeOwned>(valid: &str, with_extra: &str, extra: &str) {
    serde_json::from_str::<T>(valid).unwrap_or_else(|e| panic!("{valid}: {e}"));
    let err = match serde_json::from_str::<T>(with_extra) {
        Ok(_) => panic!("{with_extra} was accepted"),
        Err(e) => e.to_string(),
    };
    assert!(err.contains(&format!("unknown field `{extra}`")), "{err}");
}

#[test]
fn request_bodies_reject_unknown_fields() {
    strict::<ServicesAction>(
        r#"{"action":"stop","services":["web"]}"#,
        r#"{"action":"stop","services":[],"force":true}"#,
        "force",
    );
    strict::<DaemonActionRequest>(
        r#"{"action":"restart"}"#,
        r#"{"action":"restart","graceful":false}"#,
        "graceful",
    );
    strict::<SignalsRequest>(
        r#"{"signal":"SIGHUP","services":["web"]}"#,
        r#"{"signal":"SIGHUP","services":["web"],"service":"db"}"#,
        "service",
    );
    strict::<CheckRefreshRequest>(r#"{"name":"db"}"#, r#"{"name":"db","names":[]}"#, "names");
    strict::<RemoveIdentitiesRequest>(
        r#"{"identities":["ops"]}"#,
        r#"{"identities":["ops"],"all":true}"#,
        "all",
    );
    strict::<AddIdentitiesRequest>(
        r#"{"identities":{"ops":{"access":"admin","local":{}}}}"#,
        r#"{"identities":{},"replace":true}"#,
        "replace",
    );
    // Inside an identity, and inside its auth blocks.
    strict::<AddIdentitiesRequest>(
        r#"{"identities":{"ops":{"access":"admin","local":{}}}}"#,
        r#"{"identities":{"ops":{"access":"admin","local":{},"acces":"read"}}}"#,
        "acces",
    );
    strict::<AddIdentitiesRequest>(
        r#"{"identities":{"ci":{"access":"write","basic":{"password-hash":"$6$x$y"}}}}"#,
        r#"{"identities":{"ci":{"access":"write","basic":{"password-hash":"$6$x$y","password":"pw"}}}}"#,
        "password",
    );
    strict::<AddIdentitiesRequest>(
        r#"{"identities":{"ci":{"access":"write","tls":{"ca-cert":"PEM"}}}}"#,
        r#"{"identities":{"ci":{"access":"write","tls":{"ca-cert":"PEM","ca":"/x.pem"}}}}"#,
        "ca",
    );
}
