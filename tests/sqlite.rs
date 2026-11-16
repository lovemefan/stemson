//! SQLite integration tests use isolated files and run in the default suite.

use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::sqlite::SqliteArguments;
use sqlx::Arguments;
use stemson::datasource::SqliteDataSource;
use stemson::repository::{NewUserRecord, UserRepository};
use stemson::{AppContext, AppState, Config};
use tower::ServiceExt;

struct Fixture {
    directory: PathBuf,
    context: Arc<AppContext>,
    sqlite: Arc<SqliteDataSource>,
}

impl Fixture {
    fn new(in_memory: bool) -> Self {
        let directory =
            std::env::temp_dir().join(format!("stemson-sqlite-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = if in_memory {
            PathBuf::from(":memory:")
        } else {
            directory.join("test.db")
        };
        let path = directory.join("config.yaml");
        let data = json!({
            "server": {"auth": {"mode": "password"}, "jwt": {"secret": "sqlite-test-secret"}},
            "datasource": {"driver": "sqlite", "sqlite": {"path": database, "max_usage": 3, "acquire_timeout": 2}}
        });
        std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
        let context = AppContext::new(Config::load(&path).unwrap());
        context.build_components().unwrap();
        let sqlite = context.resolve::<SqliteDataSource>().unwrap();
        Self {
            directory,
            context,
            sqlite,
        }
    }

    async fn initialize(&self) {
        sqlx::raw_sql(include_str!("../sqlite/create_database.sql"))
            .execute(self.sqlite.pool())
            .await
            .unwrap();
    }

    async fn close(self) {
        self.sqlite.pool().close().await;
        std::fs::remove_dir_all(self.directory).unwrap();
    }
}

#[tokio::test]
async fn sqlite_repository_and_datasource() {
    let fixture = Fixture::new(false);
    let context = fixture.context.clone();
    let sqlite = fixture.sqlite.clone();
    let repository = context.resolve::<UserRepository>().unwrap();
    assert!(context.resolve_by_name("sqlite").is_some());
    assert!(!fixture.directory.join("test.db").exists());
    assert_eq!(sqlite.execute_many("unused", vec![]).await.unwrap(), 0);
    assert!(!fixture.directory.join("test.db").exists());
    fixture.initialize().await;
    sqlite.ping().await.unwrap();
    let (mode,): (String,) = sqlite
        .fetch_one(sqlx::query_as("PRAGMA journal_mode"))
        .await
        .unwrap();
    assert_eq!(mode, "wal");

    // The real router must authenticate using the SQLite seed hash and
    // decode signed IDs, role casts, nullable profile fields and timestamps.
    let router = stemson::build_router(AppState::new(context.clone()));
    let response = router
        .oneshot(
            Request::post("/v1/api/auth")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"username":"admin","password":"admin123"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["code"], "0000");
    assert!(body["data"]["access_token"].as_str().is_some());
    let admin = repository.find_by_username("admin").await.unwrap().unwrap();
    assert_eq!(admin.identity, 1);
    assert!(admin.create_time.is_some());
    assert!(admin.last_login_time.is_some());
    assert!(repository
        .find_by_username("' OR 1=1 --")
        .await
        .unwrap()
        .is_none());

    let uid = Utc::now().timestamp_micros();
    let username = format!("sqlite_{}", uid % 1_000_000_000_000);
    let mut record = NewUserRecord {
        uid,
        username: username.clone(),
        password_hash: Some("test-hash".into()),
        phone: None,
        email: Some("sqlite@example.com".into()),
        role: 0,
        identity: 3,
        create_by: admin.uid,
    };
    repository.insert_user_and_group(&record).await.unwrap();
    let user = repository.find_by_uid(uid).await.unwrap().unwrap();
    assert_eq!(user.username, username);
    assert_eq!(user.create_by.as_deref(), Some("admin"));
    assert_eq!(user.phone, None);
    assert_eq!(user.email.as_deref(), Some("sqlite@example.com"));
    assert_eq!(user.user_role, 0);
    assert_eq!(user.identity, 3);
    assert!(repository
        .list_all()
        .await
        .unwrap()
        .iter()
        .any(|user| user.uid == uid));
    let (count,): (i64,) = sqlite
        .fetch_one(sqlx::query_as("SELECT COUNT(*) FROM group_user WHERE uid = ?1").bind(uid))
        .await
        .unwrap();
    assert_eq!(count, 1);

    record.uid += 1;
    let error = repository.insert_user_and_group(&record).await.unwrap_err();
    assert_eq!(error.status, StatusCode::CONFLICT);
    assert!(repository.find_by_uid(record.uid).await.unwrap().is_none());
    assert_eq!(
        repository
            .update_profile(uid, "admin", None, None, 0, 1)
            .await
            .unwrap_err()
            .status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        repository
            .update_profile(uid, &username, Some("12345678901"), None, 1, 0)
            .await
            .unwrap(),
        1
    );
    let user = repository.find_by_uid(uid).await.unwrap().unwrap();
    assert_eq!(user.phone.as_deref(), Some("12345678901"));
    assert_eq!(user.email, None);
    assert_eq!(user.user_role, 1);
    assert_eq!(user.status, 0);
    assert_eq!(
        repository.update_password(uid, "new-hash").await.unwrap(),
        1
    );
    assert_eq!(
        repository
            .login_material(&username)
            .await
            .unwrap()
            .unwrap()
            .password
            .as_deref(),
        Some("new-hash")
    );

    let token = "a".repeat(64);
    repository
        .set_invite(uid, &token, (Utc::now() - Duration::minutes(1)).naive_utc())
        .await
        .unwrap();
    assert_eq!(
        repository
            .consume_invite(&username, &token, b"expired")
            .await
            .unwrap(),
        0
    );
    repository
        .set_invite(uid, &token, (Utc::now() + Duration::minutes(1)).naive_utc())
        .await
        .unwrap();
    assert_eq!(
        repository
            .consume_invite(&username, "wrong-token", b"wrong")
            .await
            .unwrap(),
        0
    );
    let registration = b"\0\xffopaque-record";
    assert_eq!(
        repository
            .consume_invite(&username, &token, registration)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repository
            .consume_invite(&username, &token, registration)
            .await
            .unwrap(),
        0
    );
    let material = repository.login_material(&username).await.unwrap().unwrap();
    assert_eq!(material.password, None);
    assert_eq!(
        material.opaque_registration.as_deref(),
        Some(registration.as_slice())
    );

    // Failure while copying groups must roll back the earlier user insertion.
    sqlite
        .execute(sqlx::query("INSERT INTO group_user (gid, uid) VALUES (1, ?1)").bind(record.uid))
        .await
        .unwrap();
    record.username = format!("rollback_{}", uid % 1_000_000_000);
    assert!(repository.insert_user_and_group(&record).await.is_err());
    assert!(repository.find_by_uid(record.uid).await.unwrap().is_none());

    // Native batch arguments retain typed NULLs, bytes, floats and booleans.
    let table = format!("stemson_batch_{uid}");
    sqlite.execute(sqlx::query(&format!(
        "CREATE TABLE {table} (id INTEGER PRIMARY KEY, n INTEGER, text TEXT, bytes BLOB, f REAL, b BOOLEAN)"
    ))).await.unwrap();
    let insert = format!("INSERT INTO {table} VALUES (?1, ?2, ?3, ?4, ?5, ?6)");
    let arguments = |id: i64| {
        let mut args = SqliteArguments::<'static>::default();
        args.add(id).unwrap();
        args.add(None::<i64>).unwrap();
        args.add("Tom or Jerry ' ? ?1").unwrap();
        args.add(vec![0u8, 255]).unwrap();
        args.add(1.25f64).unwrap();
        args.add(true).unwrap();
        args
    };
    assert_eq!(sqlite.execute_many(&insert, vec![]).await.unwrap(), 0);
    assert_eq!(
        sqlite
            .execute_many(&insert, vec![arguments(1)])
            .await
            .unwrap(),
        1
    );
    assert!(sqlite
        .execute_many(&insert, vec![arguments(2), arguments(1)])
        .await
        .is_err());
    #[derive(Debug, PartialEq, sqlx::FromRow)]
    struct BatchRow {
        id: i64,
        n: Option<i64>,
        text: String,
        bytes: Vec<u8>,
        f: f64,
        b: bool,
    }
    let rows: Vec<BatchRow> = sqlite
        .fetch_all(sqlx::query_as(&format!(
            "SELECT * FROM {table} ORDER BY id"
        )))
        .await
        .unwrap();
    assert_eq!(
        rows,
        vec![BatchRow {
            id: 1,
            n: None,
            text: "Tom or Jerry ' ? ?1".into(),
            bytes: vec![0, 255],
            f: 1.25,
            b: true,
        }]
    );
    assert!(sqlite
        .fetch_optional::<(i64,)>(sqlx::query_as(&format!(
            "SELECT id FROM {table} WHERE id = 2"
        )))
        .await
        .unwrap()
        .is_none());
    let mut tx = sqlite.begin().await.unwrap();
    sqlx::query_with(&insert, arguments(3))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let (count,): (i64,) = sqlite
        .fetch_one(sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}")))
        .await
        .unwrap();
    assert_eq!(count, 1);
    sqlite
        .execute(sqlx::query(&format!("DROP TABLE {table}")))
        .await
        .unwrap();

    assert_eq!(repository.delete(uid).await.unwrap(), 1);
    assert!(repository.find_by_uid(uid).await.unwrap().is_none());
    sqlite
        .execute(
            sqlx::query("DELETE FROM group_user WHERE uid IN (?1, ?2)")
                .bind(uid)
                .bind(record.uid),
        )
        .await
        .unwrap();
    fixture.close().await;
}

#[tokio::test]
async fn sqlite_file_survives_pool_recreation() {
    let fixture = Fixture::new(false);
    fixture.initialize().await;
    fixture.sqlite.pool().close().await;
    let context = AppContext::new(Config::load(fixture.directory.join("config.yaml")).unwrap());
    context.build_components().unwrap();
    let repository = context.resolve::<UserRepository>().unwrap();
    let admin = repository.find_by_username("admin").await.unwrap().unwrap();
    assert_eq!(admin.uid, 1341983140255768576);
    context
        .resolve::<SqliteDataSource>()
        .unwrap()
        .pool()
        .close()
        .await;
    fixture.close().await;
}

#[tokio::test]
async fn sqlite_in_memory_database_is_shared_within_pool_and_isolated_between_apps() {
    let first = Fixture::new(true);
    let second = Fixture::new(true);
    first.initialize().await;
    // max_usage is set to 3 in the fixture but memory pools must use one
    // non-recycled connection, keeping data visible across successive queries.
    assert_eq!(first.sqlite.pool().options().get_max_connections(), 1);
    assert_eq!(first.sqlite.pool().options().get_max_lifetime(), None);
    assert_eq!(first.sqlite.pool().options().get_idle_timeout(), None);
    let repository = first.context.resolve::<UserRepository>().unwrap();
    assert!(repository
        .find_by_username("admin")
        .await
        .unwrap()
        .is_some());
    let (count,): (i64,) = second
        .sqlite
        .fetch_one(sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'user'",
        ))
        .await
        .unwrap();
    assert_eq!(count, 0);
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn sqlite_invitation_can_only_be_consumed_once_concurrently() {
    let fixture = Fixture::new(false);
    fixture.initialize().await;
    let repository = fixture.context.resolve::<UserRepository>().unwrap();
    let uid = 1341983140255768576;
    let token = "b".repeat(64);
    repository
        .set_invite(uid, &token, (Utc::now() + Duration::minutes(1)).naive_utc())
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        repository.consume_invite("admin", &token, b"first"),
        repository.consume_invite("admin", &token, b"second"),
    );
    assert_eq!(first.unwrap() + second.unwrap(), 1);
    fixture.close().await;
}
