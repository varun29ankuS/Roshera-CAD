// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Task 72 — a logout survives a restart, and a refresh token can be spent
//! once.
//!
//! Before this task `AuthManager` held its revocation list only in memory.
//! With a stable `ROSHERA_JWT_SECRET` (production) a restart emptied the
//! list, so every logged-out or already-rotated refresh token verified again
//! until its own expiry — seven days by default. Rotation was also
//! check-then-insert, so two concurrent refreshes presenting one token could
//! both succeed.
//!
//! A restart is modelled the way `tests/api_key_persistence.rs` models it: a
//! FILE-backed SQLite database that outlives the `AuthManager` that wrote it,
//! and a second `AuthManager` with the SAME secret (the production
//! condition — with a different secret every old token dies on signature
//! and the test would prove nothing).
//!
//! - (a) [`a_retired_refresh_token_stays_retired_across_a_restart`]
//! - (b) [`logout_survives_a_restart`] and
//!   [`logout_after_a_renewal_survives_a_restart`]
//! - (c) [`a_second_instance_cannot_spend_a_token_the_first_already_spent`]
//!   (across instances) and
//!   [`concurrent_rotations_of_one_token_in_one_process_admit_exactly_one`]
//!   (in process, interleaving driven by a gate, not by sleeps)
//! - (d) [`a_logout_that_cannot_be_recorded_is_refused_and_changes_nothing`]
//!   and [`a_rotation_that_cannot_be_recorded_is_refused_and_does_not_spend_the_token`]

use async_trait::async_trait;
use session_manager::{
    ApiKey, AuthConfig, AuthManager, BranchRecord, CheckpointRecord, DatabaseConfig,
    DatabasePersistence, DatabaseType, DocumentRecord, NotebookRecord, ObjectMetadata,
    PrincipalKind, RevokedTokenRecord, SessionError, SessionMetadata, SessionState, SessionToken,
    SqliteDatabase, TimelineEventData, UserData, UserPermissions,
};
use shared_types::{CADObject, GeometryId};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// The secret both "boots" share — a stable `ROSHERA_JWT_SECRET`.
const SECRET: &str = "stable-production-secret";

/// Open (creating if absent) a file-backed SQLite database and run
/// migrations. A FILE — not `sqlite::memory:` — because an in-memory DB is
/// per-connection and dies with the process, so it cannot model a restart.
async fn open_db(path: &str) -> Arc<SqliteDatabase> {
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

fn db_path(dir: &tempfile::TempDir) -> String {
    dir.path()
        .join("auth.db")
        .to_string_lossy()
        .replace('\\', "/")
}

/// One process boot: a fresh `AuthManager` over `store`, restored exactly as
/// production boot restores it.
async fn boot(store: Arc<dyn DatabasePersistence>) -> AuthManager {
    let auth = AuthManager::new(AuthConfig::default(), SECRET).expect("auth manager");
    auth.attach_api_key_store(store);
    auth.load_persisted_revocations()
        .await
        .expect("the revocation list must restore at boot");
    auth
}

/// A boot whose store refuses every revocation WRITE (reads still work).
///
/// Rotating through it is a non-spending probe of the RESTORED in-memory
/// list through the one public refresh path: a token the list holds is
/// refused `AccessDenied` before the store is reached; a token it does not
/// hold reaches the store and fails `PersistenceError`, which withdraws the
/// claim. Either way nothing is spent.
async fn boot_probe(path: &str) -> AuthManager {
    let gate = GatedStore::new(open_db(path).await);
    gate.fail_revocation_writes.store(true, Ordering::SeqCst);
    boot(gate).await
}

/// Is `refresh` refused by `auth`'s in-memory revocation list? Only
/// meaningful on a [`boot_probe`] manager (see there).
async fn refused_in_memory(auth: &AuthManager, refresh: &str) -> Result<(), String> {
    match auth.rotate_refresh_token(refresh).await {
        Err(SessionError::AccessDenied) => Ok(()),
        other => Err(format!("{:?}", other.map(|t| t.id))),
    }
}

fn mint(auth: &AuthManager, user: &str) -> SessionToken {
    auth.create_token(user, None, vec!["user".to_string()], PrincipalKind::Human)
        .expect("token must mint")
}

fn refresh_of(token: &SessionToken) -> String {
    token
        .refresh_token
        .clone()
        .expect("create_token always mints a refresh token")
}

// ---------------------------------------------------------------------
// (a) + (b): revocations survive a restart
// ---------------------------------------------------------------------

#[tokio::test]
async fn a_retired_refresh_token_stays_retired_across_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(&dir);

    let r1 = {
        let auth = boot(open_db(&path).await).await;
        let first = mint(&auth, "user-a");
        let r1 = refresh_of(&first);
        auth.rotate_refresh_token(&r1)
            .await
            .expect("a fresh refresh token must rotate");
        r1
    };

    // Boot 2, before the revocation list is restored: the spent token is a
    // validly signed, unexpired JWT, so this is exactly what an empty list
    // would let through. This proves the restore below is load-bearing.
    // (A manager with no store attached: this spends nothing durable.)
    let unrestored = AuthManager::new(AuthConfig::default(), SECRET).expect("auth manager");
    assert!(
        unrestored.rotate_refresh_token(&r1).await.is_ok(),
        "sanity: without the restored list the spent token would rotate"
    );

    // The restored in-memory list refuses it on its own...
    if let Err(got) = refused_in_memory(&boot_probe(&path).await, &r1).await {
        panic!(
            "the spend recorded before the restart must be back in the revocation list, got {got}"
        );
    }
    // ...and the exchange itself is refused.
    let auth = boot(open_db(&path).await).await;
    match auth.rotate_refresh_token(&r1).await {
        Err(SessionError::AccessDenied) => {}
        other => panic!(
            "a refresh token spent before the restart must not be spendable after it, got {:?}",
            other.map(|t| t.id)
        ),
    }
}

#[tokio::test]
async fn logout_survives_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(&dir);

    let (access, refresh) = {
        let auth = boot(open_db(&path).await).await;
        let token = mint(&auth, "user-b");
        // Exactly what `handlers::auth::logout` does.
        auth.revoke_token(&token.id, "user_logout", "user-b")
            .await
            .expect("a durable logout must succeed");
        (token.token.clone(), refresh_of(&token))
    };

    let auth = boot_probe(&path).await;
    if let Err(got) = refused_in_memory(&auth, &refresh).await {
        panic!("a logged-out refresh token must stay refused after a restart, got {got}");
    }
    match auth.verify_token(&access) {
        Err(SessionError::AccessDenied) => {}
        other => panic!(
            "a logged-out access token must stay refused after a restart, got {:?}",
            other
        ),
    }
}

/// Logout reaches the refresh token through the access → refresh link. That
/// link must survive a restart too, or a logout issued AFTER a restart (with
/// an access token minted before it) ends the hour of access and leaves the
/// seven-day refresh token alive — Task 2's defect brought back by a reboot.
#[tokio::test]
async fn logout_after_a_renewal_survives_a_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(&dir);

    let (second, r2) = {
        let auth = boot(open_db(&path).await).await;
        let first = mint(&auth, "user-c");
        let second = auth
            .rotate_refresh_token(&refresh_of(&first))
            .await
            .expect("a fresh refresh token must rotate");
        let r2 = refresh_of(&second);
        (second, r2)
    };

    // Restart, THEN log out with the access token minted before it.
    let auth = boot(open_db(&path).await).await;
    let claims = auth
        .verify_token(&second.token)
        .expect("the access token minted before the restart is still valid");
    auth.revoke_token(&claims.jti, "user_logout", &claims.sub)
        .await
        .expect("a durable logout must succeed");

    match auth.rotate_refresh_token(&r2).await {
        Err(SessionError::AccessDenied) => {}
        other => panic!(
            "a logout after a restart must still reach the refresh token the client holds, got {:?}",
            other.map(|t| t.id)
        ),
    }

    // And that revocation is itself durable.
    if let Err(got) = refused_in_memory(&boot_probe(&path).await, &r2).await {
        panic!("the post-restart logout must survive the next restart too, got {got}");
    }
}

/// A spent refresh token's durable row must not lapse before the token
/// does. Here the token is minted under a 14-day refresh lifetime and spent
/// by an instance configured for 1 day (an operator shortened the lifetime
/// and restarted): a row that expired on the spender's 1-day horizon would
/// be pruned at a boot 2 days later while the token still had 12 days to
/// run — and it would rotate again.
#[tokio::test]
async fn a_spend_row_outlives_its_token_even_after_lifetimes_are_shortened() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = open_db(&db_path(&dir)).await;

    let long = AuthConfig {
        refresh_expiry_seconds: 14 * 86_400,
        ..AuthConfig::default()
    };
    let minter = AuthManager::new(long, SECRET).expect("auth manager");
    let r1 = refresh_of(&mint(&minter, "user-h"));

    let short = AuthConfig {
        refresh_expiry_seconds: 86_400,
        ..AuthConfig::default()
    };
    let spender = AuthManager::new(short, SECRET).expect("auth manager");
    spender.attach_api_key_store(db.clone());
    spender
        .rotate_refresh_token(&r1)
        .await
        .expect("a fresh refresh token must rotate");

    let two_days_on = chrono::Utc::now().timestamp_millis() + 2 * 86_400_000;
    let live = db
        .load_token_revocations(two_days_on)
        .await
        .expect("revocations must load");
    assert_eq!(
        live.len(),
        1,
        "the spend must still be on file two days on, while the 14-day token is still valid"
    );
}

// ---------------------------------------------------------------------
// (c): a refresh token can be spent once
// ---------------------------------------------------------------------

/// Two server instances over one database: each has its own in-memory
/// revocation list, so only the store can know that the other one already
/// spent the token.
#[tokio::test]
async fn a_second_instance_cannot_spend_a_token_the_first_already_spent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(&dir);

    let instance_a = boot(open_db(&path).await).await;
    let instance_b = boot(open_db(&path).await).await;

    let r1 = refresh_of(&mint(&instance_a, "user-d"));

    instance_a
        .rotate_refresh_token(&r1)
        .await
        .expect("the first presentation must rotate");

    match instance_b.rotate_refresh_token(&r1).await {
        Err(SessionError::AccessDenied) => {}
        other => panic!(
            "a token spent on one instance must not be spendable on another, got {:?}",
            other.map(|t| t.id)
        ),
    }
}

/// Two rotations of ONE token inside ONE process. The first is parked inside
/// its durable write (after it has claimed the token in memory, before the
/// store answers); the second runs to completion while it is parked. The
/// in-memory claim is the single point of truth in process, so the second
/// must be refused WITHOUT reaching the store at all.
#[tokio::test]
async fn concurrent_rotations_of_one_token_in_one_process_admit_exactly_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(&dir);
    let gate = GatedStore::new(open_db(&path).await);

    let auth = Arc::new(boot(gate.clone()).await);
    let r1 = refresh_of(&mint(&auth, "user-e"));

    gate.park_next_revocation_write();
    let first = {
        let auth = auth.clone();
        let r1 = r1.clone();
        tokio::spawn(async move { auth.rotate_refresh_token(&r1).await })
    };
    gate.parked.notified().await;

    let second = auth.rotate_refresh_token(&r1).await;
    assert_eq!(
        gate.revocation_writes.load(Ordering::SeqCst),
        1,
        "the losing rotation must be refused in memory, before it reaches the store"
    );

    gate.release.notify_one();
    let first = first.await.expect("rotation task must not panic");

    match (&first, &second) {
        (Ok(_), Err(SessionError::AccessDenied)) => {}
        _ => panic!(
            "exactly one of two concurrent rotations may succeed, got first={:?} second={:?}",
            first.as_ref().map(|t| &t.id),
            second.as_ref().map(|t| &t.id)
        ),
    }
}

// ---------------------------------------------------------------------
// (d): a revocation that is not durable is not reported as done
// ---------------------------------------------------------------------

#[tokio::test]
async fn a_logout_that_cannot_be_recorded_is_refused_and_changes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(&dir);
    let gate = GatedStore::new(open_db(&path).await);

    let auth = boot(gate.clone()).await;
    let token = mint(&auth, "user-f");
    let refresh = refresh_of(&token);

    gate.fail_revocation_writes.store(true, Ordering::SeqCst);
    match auth.revoke_token(&token.id, "user_logout", "user-f").await {
        Err(SessionError::PersistenceError { reason }) => {
            assert!(
                reason.contains(INJECTED_FAILURE),
                "the refusal must carry the store's reason, got {reason}"
            );
            assert!(!reason.contains("  "), "reason: {reason:?}");
        }
        other => panic!(
            "a logout the store did not record must be refused, got {:?}",
            other
        ),
    }

    // No half-logout: the session the caller was told is still active IS
    // still active, in full.
    assert!(
        auth.verify_token(&token.token).is_ok(),
        "a refused logout must leave the access token valid"
    );
    gate.fail_revocation_writes.store(false, Ordering::SeqCst);
    auth.rotate_refresh_token(&refresh)
        .await
        .expect("a refused logout must leave the refresh token valid");
}

#[tokio::test]
async fn a_rotation_that_cannot_be_recorded_is_refused_and_does_not_spend_the_token() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = db_path(&dir);
    let gate = GatedStore::new(open_db(&path).await);

    let auth = boot(gate.clone()).await;
    let r1 = refresh_of(&mint(&auth, "user-g"));

    gate.fail_revocation_writes.store(true, Ordering::SeqCst);
    match auth.rotate_refresh_token(&r1).await {
        Err(SessionError::PersistenceError { .. }) => {}
        other => panic!(
            "a rotation the store did not record must be refused as a persistence failure, got {:?}",
            other.map(|t| t.id)
        ),
    }

    // The token was not spent: once the store recovers it rotates normally.
    gate.fail_revocation_writes.store(false, Ordering::SeqCst);
    auth.rotate_refresh_token(&r1)
        .await
        .expect("a token whose rotation was refused must still be spendable once");
}

// ---------------------------------------------------------------------
// The store contract, one body for both backends
// ---------------------------------------------------------------------

/// What `AuthManager` relies on from `record_token_revocations`,
/// `load_token_revocations` and `prune_token_revocations`. One body, run
/// against every backend it can reach, so SQLite and PostgreSQL cannot drift
/// apart on it (Task 86).
async fn revocation_store_contract(db: &dyn DatabasePersistence, tag: &str) {
    // The real clock, not a fixed future instant: the prune below then
    // removes only rows production boot would remove anyway, so pointing
    // the Postgres run at a database holding live revocations cannot
    // delete them.
    let now = chrono::Utc::now().timestamp_millis();
    let live = RevokedTokenRecord {
        jti: format!("{tag}-live"),
        revoked_at_ms: now - 1_000,
        expires_at_ms: now + 60_000,
    };
    let lapsed = RevokedTokenRecord {
        jti: format!("{tag}-lapsed"),
        revoked_at_ms: now - 120_000,
        expires_at_ms: now - 60_000,
    };

    // Insert-if-absent, answered per record, in order.
    let first = db
        .record_token_revocations(&[live.clone(), lapsed.clone()])
        .await
        .expect("fresh revocations must record");
    assert_eq!(first, vec![true, true], "both rows are new");

    let again = RevokedTokenRecord {
        revoked_at_ms: now,
        expires_at_ms: now + 999_999,
        ..live.clone()
    };
    let second = db
        .record_token_revocations(std::slice::from_ref(&again))
        .await
        .expect("a repeated revocation is not an error");
    assert_eq!(
        second,
        vec![false],
        "a jti already on file must report that THIS call did not write it"
    );

    // The first write wins; a repeat does not overwrite it.
    let loaded = db
        .load_token_revocations(now)
        .await
        .expect("revocations must load");
    let mine: Vec<&RevokedTokenRecord> = loaded.iter().filter(|r| r.jti.starts_with(tag)).collect();
    assert_eq!(
        mine,
        vec![&live],
        "only unexpired rows load, exactly as first written"
    );

    // Pruning removes the lapsed row and nothing else.
    let pruned = db
        .prune_token_revocations(now)
        .await
        .expect("prune must succeed");
    assert!(
        pruned >= 1,
        "the lapsed row must be pruned (pruned {pruned})"
    );
    let after_prune = db
        .record_token_revocations(std::slice::from_ref(&lapsed))
        .await
        .expect("re-recording a pruned jti must succeed");
    assert_eq!(
        after_prune,
        vec![true],
        "a pruned row is gone from the store"
    );
    let live_again = db
        .record_token_revocations(std::slice::from_ref(&live))
        .await
        .expect("re-recording a live jti is not an error");
    assert_eq!(live_again, vec![false], "prune must not touch a live row");
}

#[tokio::test]
async fn sqlite_honours_the_revocation_store_contract() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = open_db(&db_path(&dir)).await;
    revocation_store_contract(db.as_ref(), "sqlite").await;
}

/// The same body against PostgreSQL. It needs a live server, so it runs only
/// when `ROSHERA_TEST_POSTGRES_URL` names one; otherwise it says so on
/// stdout rather than passing as though it had run.
#[tokio::test]
async fn postgres_honours_the_revocation_store_contract() {
    let url = match std::env::var("ROSHERA_TEST_POSTGRES_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            println!(
                "{}",
                concat!(
                    "SKIPPED postgres_honours_the_revocation_store_contract: ",
                    "ROSHERA_TEST_POSTGRES_URL is unset, so the PostgreSQL ",
                    "implementation was compiled but not exercised"
                )
            );
            return;
        }
    };
    let cfg = DatabaseConfig {
        db_type: DatabaseType::PostgreSQL,
        url,
        max_connections: 2,
        connect_timeout: 5,
        run_migrations: true,
    };
    let db = session_manager::PostgresDatabase::new(&cfg)
        .await
        .expect("ROSHERA_TEST_POSTGRES_URL must name a reachable server");
    let tag = format!("pg-{}", uuid::Uuid::new_v4());
    revocation_store_contract(&db, &tag).await;
}

// ---------------------------------------------------------------------
// Harness: a store that can park or fail the revocation write
// ---------------------------------------------------------------------

const INJECTED_FAILURE: &str = "injected store failure: disk quota exceeded";

/// Delegates everything to a real SQLite database, except
/// `record_token_revocations`, which it counts and can park (until
/// `release`) or fail.
struct GatedStore {
    inner: Arc<SqliteDatabase>,
    revocation_writes: AtomicUsize,
    park_next: AtomicBool,
    fail_revocation_writes: AtomicBool,
    parked: Notify,
    release: Notify,
}

impl GatedStore {
    fn new(inner: Arc<SqliteDatabase>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            revocation_writes: AtomicUsize::new(0),
            park_next: AtomicBool::new(false),
            fail_revocation_writes: AtomicBool::new(false),
            parked: Notify::new(),
            release: Notify::new(),
        })
    }

    fn park_next_revocation_write(&self) {
        self.park_next.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl DatabasePersistence for GatedStore {
    async fn record_token_revocations(
        &self,
        revocations: &[RevokedTokenRecord],
    ) -> Result<Vec<bool>, SessionError> {
        self.revocation_writes.fetch_add(1, Ordering::SeqCst);
        if self.park_next.swap(false, Ordering::SeqCst) {
            self.parked.notify_one();
            self.release.notified().await;
        }
        if self.fail_revocation_writes.load(Ordering::SeqCst) {
            return Err(SessionError::PersistenceError {
                reason: INJECTED_FAILURE.to_string(),
            });
        }
        self.inner.record_token_revocations(revocations).await
    }
    async fn load_token_revocations(
        &self,
        now_ms: i64,
    ) -> Result<Vec<RevokedTokenRecord>, SessionError> {
        self.inner.load_token_revocations(now_ms).await
    }
    async fn prune_token_revocations(&self, now_ms: i64) -> Result<u64, SessionError> {
        self.inner.prune_token_revocations(now_ms).await
    }

    async fn save_session(&self, session: &SessionState) -> Result<(), SessionError> {
        self.inner.save_session(session).await
    }
    async fn load_session(&self, session_id: &str) -> Result<SessionState, SessionError> {
        self.inner.load_session(session_id).await
    }
    async fn delete_session(&self, session_id: &str) -> Result<(), SessionError> {
        self.inner.delete_session(session_id).await
    }
    async fn list_sessions(
        &self,
        user_id: Option<&str>,
    ) -> Result<Vec<SessionMetadata>, SessionError> {
        self.inner.list_sessions(user_id).await
    }
    async fn save_object(&self, session_id: &str, object: &CADObject) -> Result<(), SessionError> {
        self.inner.save_object(session_id, object).await
    }
    async fn load_object(
        &self,
        session_id: &str,
        object_id: &GeometryId,
    ) -> Result<CADObject, SessionError> {
        self.inner.load_object(session_id, object_id).await
    }
    async fn delete_object(
        &self,
        session_id: &str,
        object_id: &GeometryId,
    ) -> Result<(), SessionError> {
        self.inner.delete_object(session_id, object_id).await
    }
    async fn list_objects(&self, session_id: &str) -> Result<Vec<ObjectMetadata>, SessionError> {
        self.inner.list_objects(session_id).await
    }
    async fn save_user(&self, user: &UserData) -> Result<(), SessionError> {
        self.inner.save_user(user).await
    }
    async fn load_user(&self, user_id: &str) -> Result<UserData, SessionError> {
        self.inner.load_user(user_id).await
    }
    async fn load_user_by_email(&self, email: &str) -> Result<UserData, SessionError> {
        self.inner.load_user_by_email(email).await
    }
    async fn update_user(&self, user: &UserData) -> Result<(), SessionError> {
        self.inner.update_user(user).await
    }
    async fn save_permissions(
        &self,
        session_id: &str,
        permissions: &UserPermissions,
    ) -> Result<(), SessionError> {
        self.inner.save_permissions(session_id, permissions).await
    }
    async fn load_permissions(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<UserPermissions, SessionError> {
        self.inner.load_permissions(session_id, user_id).await
    }
    async fn list_permissions(
        &self,
        session_id: &str,
    ) -> Result<Vec<UserPermissions>, SessionError> {
        self.inner.list_permissions(session_id).await
    }
    async fn save_token(&self, token: &SessionToken) -> Result<(), SessionError> {
        self.inner.save_token(token).await
    }
    async fn load_token(&self, token_id: &str) -> Result<SessionToken, SessionError> {
        self.inner.load_token(token_id).await
    }
    async fn delete_token(&self, token_id: &str) -> Result<(), SessionError> {
        self.inner.delete_token(token_id).await
    }
    async fn save_api_key(&self, api_key: &ApiKey) -> Result<(), SessionError> {
        self.inner.save_api_key(api_key).await
    }
    async fn load_api_key(&self, key_id: &str) -> Result<ApiKey, SessionError> {
        self.inner.load_api_key(key_id).await
    }
    async fn delete_api_key(&self, key_id: &str) -> Result<(), SessionError> {
        self.inner.delete_api_key(key_id).await
    }
    async fn load_all_api_keys(&self) -> Result<Vec<ApiKey>, SessionError> {
        self.inner.load_all_api_keys().await
    }
    async fn save_timeline_event(
        &self,
        session_id: &str,
        event: &TimelineEventData,
    ) -> Result<(), SessionError> {
        self.inner.save_timeline_event(session_id, event).await
    }
    async fn load_timeline_events(
        &self,
        session_id: &str,
        start: i64,
        end: i64,
    ) -> Result<Vec<TimelineEventData>, SessionError> {
        self.inner
            .load_timeline_events(session_id, start, end)
            .await
    }
    async fn get_event_count(&self, session_id: &str) -> Result<i64, SessionError> {
        self.inner.get_event_count(session_id).await
    }
    async fn load_all_timeline_events(
        &self,
        session_id: &str,
    ) -> Result<Vec<TimelineEventData>, SessionError> {
        self.inner.load_all_timeline_events(session_id).await
    }
    async fn save_branch(&self, branch: &BranchRecord) -> Result<(), SessionError> {
        self.inner.save_branch(branch).await
    }
    async fn load_branches(&self, session_id: &str) -> Result<Vec<BranchRecord>, SessionError> {
        self.inner.load_branches(session_id).await
    }
    async fn discard_redo_tail(
        &self,
        session_id: &str,
        purged_sequences: &[i64],
        branches: &[BranchRecord],
        event: &TimelineEventData,
    ) -> Result<(), SessionError> {
        self.inner
            .discard_redo_tail(session_id, purged_sequences, branches, event)
            .await
    }
    async fn save_checkpoint(&self, checkpoint: &CheckpointRecord) -> Result<(), SessionError> {
        self.inner.save_checkpoint(checkpoint).await
    }
    async fn load_checkpoints(
        &self,
        session_id: &str,
    ) -> Result<Vec<CheckpointRecord>, SessionError> {
        self.inner.load_checkpoints(session_id).await
    }
    async fn save_blackboard_notebook(
        &self,
        notebook: &NotebookRecord,
    ) -> Result<(), SessionError> {
        self.inner.save_blackboard_notebook(notebook).await
    }
    async fn load_blackboard_notebooks(
        &self,
        session_id: &str,
    ) -> Result<Vec<NotebookRecord>, SessionError> {
        self.inner.load_blackboard_notebooks(session_id).await
    }
    async fn save_document(&self, document: &DocumentRecord) -> Result<(), SessionError> {
        self.inner.save_document(document).await
    }
    async fn load_documents(&self) -> Result<Vec<DocumentRecord>, SessionError> {
        self.inner.load_documents().await
    }
    async fn delete_document(&self, id: &str) -> Result<(), SessionError> {
        self.inner.delete_document(id).await
    }
}
