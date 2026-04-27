//! # RBAC Integration Tests
//!
//! Testes de integração para controle de acesso granular (RBAC) e audit trail.

use std::net::SocketAddr;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;

use ferres_db_server::auth;
use ferres_db_server::bootstrap::{bootstrap_state, build_app};
use ferres_db_server::permissions::{Action, MetadataRestriction, Permission, Resource};
use ferres_db_server::state::ServerConfig;
use ferres_db_server::users::{Role, UserStore};

const TEST_API_KEY: &str = "rbac-test-key-123";

// ─── Test Helpers ───────────────────────────────────────────────────────

struct TestServer {
    base_url: String,
    _temp_dir: TempDir,
    _shutdown: oneshot::Sender<()>,
    #[allow(dead_code)]
    user_store: Arc<UserStore>,
}

async fn setup_server() -> TestServer {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();
    drop(listener);

    let temp_dir = tempfile::tempdir().unwrap();
    let storage_path = temp_dir.path().to_path_buf();

    let config = ServerConfig {
        host: "127.0.0.1".to_string(),
        port,
        storage_path: storage_path.clone(),
        log_level: "error".to_string(),
        api_keys: Some(TEST_API_KEY.to_string()),
        ..Default::default()
    };

    let app_state = bootstrap_state(&config).unwrap();

    // Override JWT secret for this test suite (bootstrap_state already consumed env var)
    auth::set_jwt_secret(b"test-jwt-secret-rbac".to_vec());

    // Add test-specific users (bootstrap_state already called ensure_default_user)
    if let Some(user_store) = &app_state.user_store {
        // Create a Viewer user with NO granular permissions (legacy behavior)
        user_store
            .create("viewer_user", "viewer_pass", Some(Role::Viewer))
            .unwrap();

        // Create an Editor user
        user_store
            .create("editor_user", "editor_pass", Some(Role::Editor))
            .unwrap();

        // Create a Viewer user with granular permissions: Read on "test-coll" only
        user_store
            .create_with_permissions(
                "restricted_viewer",
                "restricted_pass",
                Some(Role::Viewer),
                Some(vec![Permission {
                    resource: Resource::Collection("test-coll".to_string()),
                    actions: vec![Action::Read],
                    metadata_restriction: None,
                }]),
            )
            .unwrap();

        // Create a Viewer user with metadata restriction
        user_store
            .create_with_permissions(
                "filtered_viewer",
                "filtered_pass",
                Some(Role::Viewer),
                Some(vec![Permission {
                    resource: Resource::AllCollections,
                    actions: vec![Action::Read],
                    metadata_restriction: Some(MetadataRestriction {
                        field: "department".to_string(),
                        allowed_values: vec![serde_json::json!("sales")],
                    }),
                }]),
            )
            .unwrap();

        // Create an Editor user with Write only on specific collection
        user_store
            .create_with_permissions(
                "restricted_editor",
                "restricted_edit_pass",
                Some(Role::Viewer), // role is Viewer but has granular Write perm
                Some(vec![Permission {
                    resource: Resource::Collection("test-coll".to_string()),
                    actions: vec![Action::Read, Action::Write],
                    metadata_restriction: None,
                }]),
            )
            .unwrap();
    }

    let user_store = app_state.user_store.as_ref().unwrap().clone();
    let app = build_app(&config, app_state);

    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async {
            shutdown_rx.await.ok();
        });
        server.await.unwrap();
    });

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    TestServer {
        base_url: format!("http://127.0.0.1:{port}"),
        _temp_dir: temp_dir,
        _shutdown: shutdown_tx,
        user_store,
    }
}

/// Helper: login and get JWT token
async fn login(client: &reqwest::Client, base_url: &str, username: &str, password: &str) -> String {
    let res = client
        .post(format!("{base_url}/api/v1/auth/login"))
        .json(&serde_json::json!({"username": username, "password": password}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "login failed for {username}");
    let body: serde_json::Value = res.json().await.unwrap();
    body["token"].as_str().unwrap().to_string()
}

/// Helper: create a test collection using admin API key
async fn create_test_collection(client: &reqwest::Client, base_url: &str, name: &str) {
    let res = client
        .post(format!("{base_url}/api/v1/collections"))
        .header("Authorization", format!("Bearer {TEST_API_KEY}"))
        .json(&serde_json::json!({
            "name": name,
            "dimension": 3,
            "distance": "Cosine"
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success() || res.status() == 409,
        "failed to create collection {}: {}",
        name,
        res.status()
    );
}

/// Helper: upsert test points
async fn upsert_test_points(client: &reqwest::Client, base_url: &str, collection: &str) {
    let res = client
        .post(format!("{base_url}/api/v1/collections/{collection}/points"))
        .header("Authorization", format!("Bearer {TEST_API_KEY}"))
        .json(&serde_json::json!({
            "points": [
                {"id": "p1", "vector": [0.1, 0.2, 0.3], "metadata": {"department": "sales", "category": "A"}},
                {"id": "p2", "vector": [0.4, 0.5, 0.6], "metadata": {"department": "engineering", "category": "B"}},
                {"id": "p3", "vector": [0.7, 0.8, 0.9], "metadata": {"department": "sales", "category": "C"}}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_success(), "failed to upsert points");
}

/// Same logical points as [`upsert_test_points`], plus extra engineering rows so the
/// HNSW graph is dense enough for filtered ANN search to reliably return both `sales` hits.
async fn upsert_test_points_for_metadata_filter(
    client: &reqwest::Client,
    base_url: &str,
    collection: &str,
) {
    let res = client
        .post(format!("{base_url}/api/v1/collections/{collection}/points"))
        .header("Authorization", format!("Bearer {TEST_API_KEY}"))
        .json(&serde_json::json!({
            "points": [
                {"id": "p1", "vector": [0.1, 0.2, 0.3], "metadata": {"department": "sales", "category": "A"}},
                {"id": "p2", "vector": [0.4, 0.5, 0.6], "metadata": {"department": "engineering", "category": "B"}},
                {"id": "p3", "vector": [0.7, 0.8, 0.9], "metadata": {"department": "sales", "category": "C"}},
                {"id": "p4", "vector": [0.0, 1.0, 0.0], "metadata": {"department": "engineering", "category": "D"}},
                {"id": "p5", "vector": [0.0, 0.0, 1.0], "metadata": {"department": "engineering", "category": "E"}}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert!(
        res.status().is_success(),
        "failed to upsert points (metadata filter fixture)"
    );
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_viewer_cannot_upsert() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    create_test_collection(&client, &server.base_url, "test-coll").await;

    // Login as viewer
    let token = login(&client, &server.base_url, "viewer_user", "viewer_pass").await;

    // Try to upsert — should be denied (403)
    let res = client
        .post(format!(
            "{}/api/v1/collections/test-coll/points",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "points": [{"id": "p1", "vector": [0.1, 0.2, 0.3]}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "viewer should not be able to upsert");
}

#[tokio::test]
async fn test_viewer_can_search() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    create_test_collection(&client, &server.base_url, "test-coll").await;
    upsert_test_points(&client, &server.base_url, "test-coll").await;

    // Login as viewer
    let token = login(&client, &server.base_url, "viewer_user", "viewer_pass").await;

    // Search — should work (viewer has Read by default)
    let res = client
        .post(format!(
            "{}/api/v1/collections/test-coll/search",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "vector": [0.1, 0.2, 0.3],
            "limit": 10
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "viewer should be able to search");
}

#[tokio::test]
async fn test_admin_bypasses_all_restrictions() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    // Admin via API key can do everything
    create_test_collection(&client, &server.base_url, "admin-test").await;
    upsert_test_points(&client, &server.base_url, "admin-test").await;

    let res = client
        .post(format!(
            "{}/api/v1/collections/admin-test/search",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {TEST_API_KEY}"))
        .json(&serde_json::json!({
            "vector": [0.1, 0.2, 0.3],
            "limit": 10
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 3, "admin should see all 3 points");
}

#[tokio::test]
async fn test_metadata_restriction_filters_results() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    create_test_collection(&client, &server.base_url, "filtered-test").await;
    upsert_test_points_for_metadata_filter(&client, &server.base_url, "filtered-test").await;

    // Login as filtered_viewer (has metadata restriction: department=sales)
    let token = login(
        &client,
        &server.base_url,
        "filtered_viewer",
        "filtered_pass",
    )
    .await;

    // Search — should only see points with department=sales
    let res = client
        .post(format!(
            "{}/api/v1/collections/filtered-test/search",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "vector": [0.1, 0.2, 0.3],
            "limit": 10
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let results = body["results"].as_array().unwrap();

    // Should only see p1 and p3 (department=sales), not p2 (department=engineering)
    assert_eq!(
        results.len(),
        2,
        "filtered viewer should see only sales department points"
    );
    for r in results {
        let meta = &r["metadata"];
        assert_eq!(
            meta["department"].as_str().unwrap(),
            "sales",
            "all results should be from sales department"
        );
    }
}

#[tokio::test]
async fn test_granular_permissions_collection_specific() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    create_test_collection(&client, &server.base_url, "test-coll").await;
    create_test_collection(&client, &server.base_url, "other-coll").await;
    upsert_test_points(&client, &server.base_url, "test-coll").await;

    // Login as restricted_viewer (has Read only on test-coll)
    let token = login(
        &client,
        &server.base_url,
        "restricted_viewer",
        "restricted_pass",
    )
    .await;

    // Search on test-coll — should work
    let res = client
        .post(format!(
            "{}/api/v1/collections/test-coll/search",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "vector": [0.1, 0.2, 0.3],
            "limit": 10
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "restricted_viewer should read test-coll");

    // Search on other-coll — should be denied
    let res = client
        .post(format!(
            "{}/api/v1/collections/other-coll/search",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "vector": [0.1, 0.2, 0.3],
            "limit": 10
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        403,
        "restricted_viewer should NOT read other-coll"
    );
}

#[tokio::test]
async fn test_granular_write_permission() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    create_test_collection(&client, &server.base_url, "test-coll").await;
    create_test_collection(&client, &server.base_url, "other-coll").await;

    // Login as restricted_editor (has Read+Write only on test-coll)
    let token = login(
        &client,
        &server.base_url,
        "restricted_editor",
        "restricted_edit_pass",
    )
    .await;

    // Upsert to test-coll — should work
    let res = client
        .post(format!(
            "{}/api/v1/collections/test-coll/points",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "points": [{"id": "rp1", "vector": [0.1, 0.2, 0.3]}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        200,
        "restricted_editor should write to test-coll"
    );

    // Upsert to other-coll — should be denied
    let res = client
        .post(format!(
            "{}/api/v1/collections/other-coll/points",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "points": [{"id": "rp1", "vector": [0.1, 0.2, 0.3]}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        403,
        "restricted_editor should NOT write to other-coll"
    );
}

#[tokio::test]
async fn test_audit_trail_records_actions() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    create_test_collection(&client, &server.base_url, "audit-test").await;
    upsert_test_points(&client, &server.base_url, "audit-test").await;

    // Do a search with the admin API key
    let _ = client
        .post(format!(
            "{}/api/v1/collections/audit-test/search",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {TEST_API_KEY}"))
        .json(&serde_json::json!({
            "vector": [0.1, 0.2, 0.3],
            "limit": 5
        }))
        .send()
        .await
        .unwrap();

    // Wait for buffered audit writes to flush (1s flush interval + margin)
    tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

    // Query the audit trail
    let res = client
        .get(format!(
            "{}/api/v1/audit?action=search&limit=10",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {TEST_API_KEY}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "audit trail should contain search entries"
    );

    // Check that the entry has expected fields
    let entry = &entries[0];
    assert_eq!(entry["action"].as_str().unwrap(), "search");
    assert!(entry["resource"].as_str().unwrap().contains("audit-test"));
    assert_eq!(entry["result"].as_str().unwrap(), "success");
}

#[tokio::test]
async fn test_audit_trail_records_denied_actions() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    create_test_collection(&client, &server.base_url, "deny-test").await;

    // Login as viewer
    let token = login(&client, &server.base_url, "viewer_user", "viewer_pass").await;

    // Try to upsert (will be denied)
    let _ = client
        .post(format!(
            "{}/api/v1/collections/deny-test/points",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {token}"))
        .json(&serde_json::json!({
            "points": [{"id": "p1", "vector": [0.1, 0.2, 0.3]}]
        }))
        .send()
        .await
        .unwrap();

    // Wait for buffered audit writes to flush (1s flush interval + margin)
    tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;

    // Query audit trail for denied actions
    let res = client
        .get(format!(
            "{}/api/v1/audit?action=upsert&limit=10",
            server.base_url
        ))
        .header("Authorization", format!("Bearer {TEST_API_KEY}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.unwrap();
    let entries = body.as_array().unwrap();

    // Find the denied entry
    let denied = entries
        .iter()
        .find(|e| e["result"].as_str() == Some("denied"));
    assert!(denied.is_some(), "should have a denied audit entry");
    assert_eq!(denied.unwrap()["user_id"].as_str().unwrap(), "viewer_user");
}

#[tokio::test]
async fn test_audit_requires_admin() {
    let server = setup_server().await;
    let client = reqwest::Client::new();

    // Login as viewer
    let token = login(&client, &server.base_url, "viewer_user", "viewer_pass").await;

    // Try to access audit trail
    let res = client
        .get(format!("{}/api/v1/audit", server.base_url))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403, "viewer should not access audit trail");
}

// ─── Unit Tests for permissions module ──────────────────────────────────

#[test]
fn test_check_permission_logic() {
    use ferres_db_server::permissions::{check_permission, PermissionResult};

    // Empty permissions → deny
    let result = check_permission(&[], "docs", &Action::Read);
    assert!(!result.is_allowed());

    // Specific collection match
    let perms = vec![Permission {
        resource: Resource::Collection("docs".to_string()),
        actions: vec![Action::Read, Action::Write],
        metadata_restriction: None,
    }];
    assert!(check_permission(&perms, "docs", &Action::Read).is_allowed());
    assert!(check_permission(&perms, "docs", &Action::Write).is_allowed());
    assert!(!check_permission(&perms, "docs", &Action::Delete).is_allowed());
    assert!(!check_permission(&perms, "other", &Action::Read).is_allowed());

    // AllCollections wildcard
    let perms = vec![Permission {
        resource: Resource::AllCollections,
        actions: vec![Action::Read],
        metadata_restriction: None,
    }];
    assert!(check_permission(&perms, "any_coll", &Action::Read).is_allowed());
    assert!(!check_permission(&perms, "any_coll", &Action::Write).is_allowed());

    // Metadata restriction
    let perms = vec![Permission {
        resource: Resource::AllCollections,
        actions: vec![Action::Read],
        metadata_restriction: Some(MetadataRestriction {
            field: "team".to_string(),
            allowed_values: vec![serde_json::json!("alpha")],
        }),
    }];
    match check_permission(&perms, "docs", &Action::Read) {
        PermissionResult::AllowedWithRestriction(r) => {
            assert_eq!(r.field, "team");
        }
        _ => panic!("expected AllowedWithRestriction"),
    }
}

#[test]
fn test_user_store_permissions_persistence() {
    let temp_dir = tempfile::tempdir().unwrap();
    let db_path = temp_dir.path().join("test_users.db");

    let store = UserStore::new(&db_path).unwrap();
    store.ensure_default_user().unwrap();

    let perms = vec![Permission {
        resource: Resource::Collection("docs".to_string()),
        actions: vec![Action::Read],
        metadata_restriction: None,
    }];

    store
        .create_with_permissions(
            "testuser",
            "testpass",
            Some(Role::Viewer),
            Some(perms.clone()),
        )
        .unwrap();

    // Read back permissions
    let loaded = store.get_permissions("testuser").unwrap();
    assert!(loaded.is_some());
    let loaded_perms = loaded.unwrap();
    assert_eq!(loaded_perms.len(), 1);
    assert_eq!(
        loaded_perms[0].resource,
        Resource::Collection("docs".to_string())
    );

    // Update permissions
    let new_perms = vec![Permission {
        resource: Resource::AllCollections,
        actions: vec![Action::Read, Action::Write],
        metadata_restriction: None,
    }];
    store
        .update_permissions("testuser", Some(new_perms))
        .unwrap();

    let reloaded = store.get_permissions("testuser").unwrap().unwrap();
    assert_eq!(reloaded.len(), 1);
    assert_eq!(reloaded[0].resource, Resource::AllCollections);

    // Clear permissions
    store.update_permissions("testuser", None).unwrap();
    let cleared = store.get_permissions("testuser").unwrap();
    assert!(cleared.is_none());
}
