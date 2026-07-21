//! Auth Slice 3 (task #42) — API-key persistence across a restart.
//!
//! RED-first: before this slice, `AuthManager` held API keys only in an
//! in-memory `DashMap`, so a provisioned key died when the process
//! restarted. These tests model a restart the same way the durability
//! boot tests do — a file-backed SQLite database that outlives the
//! `AuthManager` that wrote it — with ZERO live infrastructure, so the
//! RED is honest and runs in CI.
//!
//! - [`provisioned_api_key_survives_simulated_restart`] is the fix: a key
//!   minted against a durable store authenticates again in a brand-new
//!   `AuthManager` after the store is reloaded.
//! - [`api_key_is_lost_without_persistence_store`] is the mutation proof:
//!   with no store attached (the pre-Slice-3 behaviour) the same key does
//!   NOT survive — pinning that the durable store is the load-bearing
//!   element, not some incidental state.
//! - [`inactive_persisted_key_stays_denied_after_restart`] guards the
//!   faithful boot-restore mapping: a key persisted as `active = false`
//!   comes back DENIED, never silently revived (the SQLite single-key
//!   `load_api_key` hardcodes `active = true`; the boot path must not).

use chrono::Utc;
use session_manager::{
    ApiKey, AuthConfig, AuthManager, DatabaseConfig, DatabasePersistence, DatabaseType,
    SqliteDatabase,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Open (creating if absent) a file-backed SQLite database and run
/// migrations. A FILE — not `sqlite::memory:` — because an in-memory DB is
/// per-connection and dies with the process, so it cannot model a restart.
async fn open_db(path: &str) -> Arc<dyn DatabasePersistence> {
    let cfg = DatabaseConfig {
        db_type: DatabaseType::SQLite,
        url: format!("sqlite://{path}?mode=rwc"),
        max_connections: 4,
        connect_timeout: 5,
        run_migrations: true,
    };
    Arc::new(
        SqliteDatabase::new(&cfg)
            .await
            .expect("file-backed sqlite must initialise"),
    )
}

/// SHA-256 hex of a raw key, exactly as `AuthManager::create_api_key`
/// computes the stored `key_hash`.
fn hash_raw(raw: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[tokio::test]
async fn provisioned_api_key_survives_simulated_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir
        .path()
        .join("auth.db")
        .to_string_lossy()
        .replace('\\', "/");

    // ---- Boot 1: provision a key against a durable store. ----
    let raw_key = {
        let db = open_db(&path).await;
        let auth = AuthManager::new(AuthConfig::default(), "boot-1-secret").expect("auth mgr");
        auth.attach_api_key_store(db.clone());

        let (raw, key) = auth
            .provision_api_key(
                "agent-1",
                "ci key",
                vec!["read".to_string(), "write".to_string()],
                None,
            )
            .await
            .expect("provision must succeed and persist");

        // Sanity: it authenticates in the booting process.
        let verified = auth.verify_api_key(&raw).expect("fresh key verifies");
        assert_eq!(verified.id, key.id);
        raw
    };

    // ---- Boot 2: a brand-new AuthManager over the SAME db file (a restart).
    // A DIFFERENT jwt secret proves API-key verification is independent of the
    // per-process random JWT secret (which does not survive a restart). ----
    let db2 = open_db(&path).await;
    let auth2 = AuthManager::new(AuthConfig::default(), "boot-2-secret").expect("auth mgr 2");
    auth2.attach_api_key_store(db2.clone());

    let restored = auth2
        .load_persisted_api_keys()
        .await
        .expect("boot restore must query the store");
    assert_eq!(
        restored, 1,
        "exactly the one provisioned key must be restored at boot"
    );

    // THE SLICE-3 ASSERTION: the key minted before the restart still
    // authenticates afterward.
    let verified = auth2
        .verify_api_key(&raw_key)
        .expect("the persisted API key MUST authenticate after a restart");
    assert_eq!(verified.user_id, "agent-1");
    assert!(verified.active, "the restored key must be active");
    assert_eq!(
        verified.permissions,
        vec!["read".to_string(), "write".to_string()],
        "permissions must survive the restart intact"
    );
    assert!(
        verified.rate_limit.is_some(),
        "the per-key rate limit (AUDIT-H9) must survive the restart"
    );
}

#[tokio::test]
async fn api_key_is_lost_without_persistence_store() {
    // Mutation proof: with NO store attached (the pre-Slice-3 behaviour —
    // in-memory DashMap only), a key minted in one manager cannot be
    // recovered by a fresh manager. This is exactly the restart-loss the
    // fix eliminates; if this ever passes with the key surviving, the test
    // above is not actually exercising persistence.
    let auth1 = AuthManager::new(AuthConfig::default(), "s1").expect("auth mgr");
    let (raw, _key) = auth1
        .provision_api_key("agent-x", "volatile", vec!["read".to_string()], None)
        .await
        .expect("provision with no store behaves like create_api_key");
    assert!(
        auth1.verify_api_key(&raw).is_ok(),
        "the key works within the process that minted it"
    );

    // A fresh manager (the restart) with no store and nothing to load.
    let auth2 = AuthManager::new(AuthConfig::default(), "s2").expect("auth mgr");
    let restored = auth2
        .load_persisted_api_keys()
        .await
        .expect("load is a no-op when no store is attached");
    assert_eq!(restored, 0, "no store attached → nothing to restore");
    assert!(
        auth2.verify_api_key(&raw).is_err(),
        "without persistence a provisioned key does NOT survive a restart"
    );
}

#[tokio::test]
async fn inactive_persisted_key_stays_denied_after_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir
        .path()
        .join("auth.db")
        .to_string_lossy()
        .replace('\\', "/");

    // Persist a key as INACTIVE (as a revoked key would be stored), hashing
    // a known raw secret so we can later attempt to authenticate with it.
    let raw = "rosh_fixture_inactive_key";
    let inactive = ApiKey {
        id: uuid::Uuid::new_v4().to_string(),
        name: "revoked fixture".to_string(),
        key_hash: hash_raw(raw),
        prefix: raw[..8].to_string(),
        user_id: "agent-revoked".to_string(),
        permissions: vec!["read".to_string()],
        rate_limit: None,
        created_at: Utc::now(),
        last_used: None,
        expires_at: None,
        active: false,
    };
    {
        let db = open_db(&path).await;
        db.save_api_key(&inactive)
            .await
            .expect("persist inactive key");
    }

    // Restart: load persisted keys into a fresh manager.
    let db2 = open_db(&path).await;
    let auth = AuthManager::new(AuthConfig::default(), "s").expect("auth mgr");
    auth.attach_api_key_store(db2.clone());
    let restored = auth
        .load_persisted_api_keys()
        .await
        .expect("boot restore must query the store");
    assert_eq!(restored, 1, "the inactive key must be loaded, not skipped");

    // FAITHFUL MAPPING GUARD: the restored key carries `active = false`, so
    // authentication is DENIED. A boot-restore that hardcoded `active = true`
    // (as the legacy single-key SQLite `load_api_key` does) would ACCEPT here
    // — silently reviving a revoked credential across a restart.
    assert!(
        auth.verify_api_key(raw).is_err(),
        "an inactive (revoked) key must stay DENIED after a restart"
    );
}
