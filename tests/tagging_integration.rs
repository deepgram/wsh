//! Integration tests for session tagging.
//!
//! These tests verify the full HTTP API flow for tag lifecycle operations:
//! - Creating sessions with tags
//! - Listing sessions filtered by tag
//! - Adding and removing tags via PATCH
//! - Tag validation (invalid tags return 400)
//! - Rename preserves tags
//! - Multiple sessions with tag filter (union semantics)
//! - Empty tag filter returns all sessions
//! - Group idle filtered by tag

use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;
use wsh::api::{router, AppState, RouterConfig};
use wsh::session::SessionRegistry;
use wsh::shutdown::ShutdownCoordinator;

/// Creates a test app with an empty session registry (no sessions pre-created).
fn create_empty_test_app() -> axum::Router {
    let registry = SessionRegistry::new();
    let state = AppState {
        sessions: registry,
        shutdown: ShutdownCoordinator::new(),
        server_config: std::sync::Arc::new(wsh::api::ServerConfig::new(false)),
        server_ws_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        mcp_session_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        ticket_store: std::sync::Arc::new(wsh::api::ticket::TicketStore::new()),
        backends: wsh::federation::registry::BackendRegistry::new(),
        federation: std::sync::Arc::new(tokio::sync::Mutex::new(wsh::federation::manager::FederationManager::new())),
        hostname: "test".to_string(),
        federation_config_path: None,
        local_token: None,
        default_backend_token: None,
    };
    router(state, RouterConfig::default())
}

async fn start_test_server(app: axum::Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(10)).await;
    addr
}

// ---------------------------------------------------------------------------
// Test 1: HTTP tag lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_http_tag_lifecycle() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // 1. Create a session with tags
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "tagged", "tags": ["build", "ci"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201, "Expected 201 Created");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "tagged");
    let tags = body["tags"].as_array().unwrap();
    let tag_strs: Vec<&str> = tags.iter().map(|t| t.as_str().unwrap()).collect();
    assert!(tag_strs.contains(&"build"));
    assert!(tag_strs.contains(&"ci"));

    // 2. List sessions with ?tag=build — should find the tagged session
    let resp = client
        .get(format!("{}/sessions?tag=build", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(body.len(), 1, "Expected 1 session with tag 'build'");
    assert_eq!(body[0]["name"], "tagged");

    // 3. List sessions with ?tag=nonexistent — should be empty
    let resp = client
        .get(format!("{}/sessions?tag=nonexistent", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(body.is_empty(), "Expected no sessions with tag 'nonexistent'");

    // 4. Add tags via PATCH with add_tags
    let resp = client
        .patch(format!("{}/sessions/tagged", base))
        .json(&serde_json::json!({"add_tags": ["deploy"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tags = body["tags"].as_array().unwrap();
    let tag_strs: Vec<&str> = tags.iter().map(|t| t.as_str().unwrap()).collect();
    assert!(tag_strs.contains(&"build"), "Should still have 'build'");
    assert!(tag_strs.contains(&"ci"), "Should still have 'ci'");
    assert!(tag_strs.contains(&"deploy"), "Should now have 'deploy'");
    assert_eq!(tags.len(), 3, "Expected exactly 3 tags");

    // 5. Remove tags via PATCH with remove_tags
    let resp = client
        .patch(format!("{}/sessions/tagged", base))
        .json(&serde_json::json!({"remove_tags": ["ci"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tags = body["tags"].as_array().unwrap();
    let tag_strs: Vec<&str> = tags.iter().map(|t| t.as_str().unwrap()).collect();
    assert!(tag_strs.contains(&"build"), "Should still have 'build'");
    assert!(!tag_strs.contains(&"ci"), "Should no longer have 'ci'");
    assert!(tag_strs.contains(&"deploy"), "Should still have 'deploy'");
    assert_eq!(tags.len(), 2, "Expected exactly 2 tags");

    // 6. Delete the session
    let resp = client
        .delete(format!("{}/sessions/tagged", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // 7. List with the old tag filter — should be empty now
    let resp = client
        .get(format!("{}/sessions?tag=build", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(body.is_empty(), "Expected no sessions after deletion");
}

// ---------------------------------------------------------------------------
// Test 2: Tag validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tag_validation_on_create() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create session with invalid tag (contains spaces)
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "bad-tags", "tags": ["valid", "has space"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "Expected 400 Bad Request for invalid tag");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["error"]["code"], "invalid_tag",
        "Expected 'invalid_tag' error code, got: {:?}",
        body
    );
}

#[tokio::test]
async fn test_tag_validation_on_create_empty_tag() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create session with empty tag
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "bad-empty", "tags": [""]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "Expected 400 for empty tag");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_tag");
}

#[tokio::test]
async fn test_tag_validation_on_patch_add() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create a valid session first
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "patch-test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Try to add an invalid tag via PATCH
    let resp = client
        .patch(format!("{}/sessions/patch-test", base))
        .json(&serde_json::json!({"add_tags": ["special!char"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "Expected 400 for invalid tag in PATCH");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_tag");
}

#[tokio::test]
async fn test_tag_validation_too_long() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Tag longer than 64 characters
    let long_tag = "x".repeat(65);
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "long-tag", "tags": [long_tag]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "Expected 400 for tag too long");
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "invalid_tag");
}

// ---------------------------------------------------------------------------
// Test 3: Rename preserves tags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rename_preserves_tags() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create session with tags
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "before-rename", "tags": ["alpha", "beta"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Rename the session
    let resp = client
        .patch(format!("{}/sessions/before-rename", base))
        .json(&serde_json::json!({"name": "after-rename"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "after-rename");
    let tags = body["tags"].as_array().unwrap();
    let tag_strs: Vec<&str> = tags.iter().map(|t| t.as_str().unwrap()).collect();
    assert!(tag_strs.contains(&"alpha"), "Tag 'alpha' should be preserved after rename");
    assert!(tag_strs.contains(&"beta"), "Tag 'beta' should be preserved after rename");

    // Verify tag filter finds the session by new name
    let resp = client
        .get(format!("{}/sessions?tag=alpha", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(body.len(), 1, "Tag filter should find the renamed session");
    assert_eq!(body[0]["name"], "after-rename");

    // Verify old name is gone from tag filter
    let names: Vec<&str> = body.iter().map(|v| v["name"].as_str().unwrap()).collect();
    assert!(!names.contains(&"before-rename"), "Old name should not appear in tag filter");
}

// ---------------------------------------------------------------------------
// Test 4: Multiple sessions with tag filter (union semantics)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multiple_sessions_tag_filter() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create 3 sessions with different tag combinations
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "s1", "tags": ["build"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "s2", "tags": ["test"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "s3", "tags": ["build", "test"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Filter by "build" — should return s1 and s3
    let resp = client
        .get(format!("{}/sessions?tag=build", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let mut names: Vec<String> = body.iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
    names.sort();
    assert_eq!(names, vec!["s1", "s3"], "tag=build should match s1 and s3");

    // Filter by "test" — should return s2 and s3
    let resp = client
        .get(format!("{}/sessions?tag=test", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let mut names: Vec<String> = body.iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
    names.sort();
    assert_eq!(names, vec!["s2", "s3"], "tag=test should match s2 and s3");

    // Filter by "build,test" (union semantics) — should return all 3
    let resp = client
        .get(format!("{}/sessions?tag=build,test", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    let mut names: Vec<String> = body.iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
    names.sort();
    assert_eq!(names, vec!["s1", "s2", "s3"], "tag=build,test should match all 3");

    // Filter by "deploy" — should return none
    let resp = client
        .get(format!("{}/sessions?tag=deploy", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(body.is_empty(), "tag=deploy should match no sessions");
}

// ---------------------------------------------------------------------------
// Test 5: Empty tag filter returns all
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_empty_tag_filter_returns_all() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create sessions — some tagged, some not
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "tagged-session", "tags": ["build"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "untagged-session"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // GET /sessions (no tag param) returns all
    let resp = client
        .get(format!("{}/sessions", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(body.len(), 2, "Should return all sessions without tag filter");

    let mut names: Vec<String> = body.iter().map(|v| v["name"].as_str().unwrap().to_string()).collect();
    names.sort();
    assert_eq!(names, vec!["tagged-session", "untagged-session"]);

    // Verify the untagged session has an empty tags array
    let untagged = body.iter().find(|v| v["name"] == "untagged-session").unwrap();
    let tags = untagged["tags"].as_array().unwrap();
    assert!(tags.is_empty(), "Untagged session should have empty tags array");

    // Verify the tagged session has its tags
    let tagged = body.iter().find(|v| v["name"] == "tagged-session").unwrap();
    let tags = tagged["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 1);
    assert_eq!(tags[0], "build");
}

// ---------------------------------------------------------------------------
// Test 6: Session info (GET /sessions/:name) includes tags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_info_includes_tags() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create session with tags
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "info-test", "tags": ["web", "prod"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // GET individual session info
    let resp = client
        .get(format!("{}/sessions/info-test", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "info-test");
    let tags = body["tags"].as_array().unwrap();
    let tag_strs: Vec<&str> = tags.iter().map(|t| t.as_str().unwrap()).collect();
    assert!(tag_strs.contains(&"web"));
    assert!(tag_strs.contains(&"prod"));
}

// ---------------------------------------------------------------------------
// Test 7: Tags are sorted in responses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_tags_sorted_in_response() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create session with unsorted tags
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "sorted-test", "tags": ["zulu", "alpha", "mike"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tags: Vec<&str> = body["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    assert_eq!(tags, vec!["alpha", "mike", "zulu"], "Tags should be sorted alphabetically");
}

// ---------------------------------------------------------------------------
// Test 8: Combine rename and tag operations in single PATCH
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_rename_and_add_tags_in_single_patch() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create session
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "combo-old", "tags": ["existing"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // PATCH with rename + add_tags + remove_tags all at once
    let resp = client
        .patch(format!("{}/sessions/combo-old", base))
        .json(&serde_json::json!({
            "name": "combo-new",
            "add_tags": ["added"],
            "remove_tags": ["existing"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "combo-new");
    let tags: Vec<&str> = body["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    assert_eq!(tags, vec!["added"], "Should have only 'added' after combo PATCH");

    // Verify old name is gone
    let resp = client
        .get(format!("{}/sessions/combo-old", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);

    // Verify new name has correct tags
    let resp = client
        .get(format!("{}/sessions/combo-new", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tags: Vec<&str> = body["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    assert_eq!(tags, vec!["added"]);
}

// ---------------------------------------------------------------------------
// Test 9: Group idle with tag filter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_group_idle_with_tag_filter() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create sessions with different tags
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "q1", "tags": ["build"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "q2", "tags": ["test"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Wait for both sessions to settle
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Group idle with tag=build — should only consider q1
    let resp = client
        .get(format!("{}/idle?timeout_ms=100&tag=build&format=plain", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["session"], "q1", "Should pick session with tag 'build'");

    // Group idle with tag=test — should only consider q2
    let resp = client
        .get(format!("{}/idle?timeout_ms=100&tag=test&format=plain", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["session"], "q2", "Should pick session with tag 'test'");

    // Group idle with tag=nonexistent — should return 404 (no sessions match)
    let resp = client
        .get(format!("{}/idle?timeout_ms=100&tag=nonexistent&format=plain", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "no_sessions");
}

// ---------------------------------------------------------------------------
// Test 10: Create session without tags — tags defaults to empty
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_create_session_without_tags_defaults_to_empty() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create session without tags field
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "no-tags"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "no-tags");
    let tags = body["tags"].as_array().unwrap();
    assert!(tags.is_empty(), "Tags should default to empty array");
}

// ---------------------------------------------------------------------------
// Test 11: Adding duplicate tags is idempotent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_add_duplicate_tags_idempotent() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create session with a tag
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "dupe", "tags": ["build"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Add the same tag again
    let resp = client
        .patch(format!("{}/sessions/dupe", base))
        .json(&serde_json::json!({"add_tags": ["build"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tags = body["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 1, "Duplicate tag should not create duplicates");
    assert_eq!(tags[0], "build");
}

// ---------------------------------------------------------------------------
// Test 12: Removing a tag that doesn't exist is a no-op
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_remove_nonexistent_tag_is_noop() {
    let app = create_empty_test_app();
    let addr = start_test_server(app).await;
    let client = reqwest::Client::new();
    let base = format!("http://{}", addr);

    // Create session with tags
    let resp = client
        .post(format!("{}/sessions", base))
        .json(&serde_json::json!({"name": "noop", "tags": ["keep"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Remove a tag that was never added
    let resp = client
        .patch(format!("{}/sessions/noop", base))
        .json(&serde_json::json!({"remove_tags": ["never-existed"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tags: Vec<&str> = body["tags"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
    assert_eq!(tags, vec!["keep"], "Existing tags should be unchanged");
}
