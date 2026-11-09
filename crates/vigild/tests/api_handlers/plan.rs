// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 vigil-rs contributors

use axum::http::StatusCode;

use super::{TestApp, body_json};

#[tokio::test]
async fn get_plan_returns_the_merged_plan_with_secrets_masked() {
    let app = TestApp::with_layer(
        "services:\n  web:\n    command: /bin/sleep 60\n    \
         environment: {DB_PASSWORD: s3cret}\n\
         alerts:\n  ops:\n    url: https://hooks.example.com/T0/secret\n    on-check: ['*']\n",
    )
    .await;
    let resp = app.get("/v1/plan").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let plan = &body_json(resp).await["result"];
    assert_eq!(plan["services"]["web"]["command"], "/bin/sleep 60");
    assert_eq!(plan["services"]["web"]["environment"]["DB_PASSWORD"], "***");
    assert_eq!(
        plan["alerts"]["ops"]["url"],
        "https://hooks.example.com/***"
    );
    assert!(plan.get("checks").is_none(), "empty sections are left out");
    app.shutdown().await;
}
