//! Requires a disposable, initialized PostgreSQL database:
//! `STEMSON_TEST_POSTGRES_CONFIG=config/config.postgres.yaml cargo test --test postgres -- --ignored`

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use serde_json::json;
use sqlx::postgres::PgArguments;
use sqlx::Arguments;
use stemson::datasource::PostgresDataSource;
use stemson::repository::{NewUserRecord, UserRepository};
use stemson::{AppContext, AppState, Config};
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires a disposable PostgreSQL database initialized with postgres/create_database.sql"]
async fn postgres_repository_and_datasource() {
    let path = std::env::var("STEMSON_TEST_POSTGRES_CONFIG")
        .unwrap_or_else(|_| "config/config.postgres.yaml".into());
    let source = Config::load(path).unwrap();
    let mut data = (*source.snapshot()).clone();
    data["server"]["auth"]["mode"] = json!("password");
    data["datasource"]["driver"] = json!("postgres");
    let directory = std::env::temp_dir().join(format!("stemson-pg-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("config.yaml");
    std::fs::write(&path, serde_json::to_vec(&data).unwrap()).unwrap();
    let context = AppContext::new(Config::load(&path).unwrap());
    context.build_components().unwrap();
    let postgres = context.resolve::<PostgresDataSource>().unwrap();
    let repository = context.resolve::<UserRepository>().unwrap();
    postgres.ping().await.unwrap();

    // The real router must authenticate using the PostgreSQL seed hash and
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
    let username = format!("pg_{}", uid % 1_000_000_000_000);
    let mut record = NewUserRecord {
        uid,
        username: username.clone(),
        password_hash: Some("test-hash".into()),
        phone: None,
        email: Some("pgsql@example.com".into()),
        role: 0,
        identity: 3,
        create_by: admin.uid,
    };
    repository.insert_user_and_group(&record).await.unwrap();
    let user = repository.find_by_uid(uid).await.unwrap().unwrap();
    assert_eq!(user.username, username);
    assert_eq!(user.create_by.as_deref(), Some("admin"));
    assert_eq!(user.phone, None);
    assert_eq!(user.email.as_deref(), Some("pgsql@example.com"));
    assert_eq!(user.user_role, 0);
    assert_eq!(user.identity, 3);
    assert!(repository
        .list_all()
        .await
        .unwrap()
        .iter()
        .any(|user| user.uid == uid));
    let (count,): (i64,) = postgres
        .fetch_one(sqlx::query_as("SELECT COUNT(*) FROM group_user WHERE uid = $1").bind(uid))
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
    postgres
        .execute(sqlx::query("INSERT INTO group_user (gid, uid) VALUES (1, $1)").bind(record.uid))
        .await
        .unwrap();
    record.username = format!("rollback_{}", uid % 1_000_000_000);
    assert!(repository.insert_user_and_group(&record).await.is_err());
    assert!(repository.find_by_uid(record.uid).await.unwrap().is_none());

    // Native batch arguments retain typed NULLs, bytes, floats and booleans.
    let table = format!("stemson_batch_{uid}");
    postgres.execute(sqlx::query(&format!(
        "CREATE TABLE {table} (id BIGINT PRIMARY KEY, n BIGINT, text TEXT, bytes BYTEA, f DOUBLE PRECISION, b BOOLEAN)"
    ))).await.unwrap();
    let insert = format!("INSERT INTO {table} VALUES ($1, $2, $3, $4, $5, $6)");
    let arguments = |id: i64| {
        let mut args = PgArguments::default();
        args.add(id).unwrap();
        args.add(None::<i64>).unwrap();
        args.add("Tom or Jerry ' ? $1").unwrap();
        args.add(vec![0u8, 255]).unwrap();
        args.add(1.25f64).unwrap();
        args.add(true).unwrap();
        args
    };
    assert_eq!(postgres.execute_many(&insert, vec![]).await.unwrap(), 0);
    assert_eq!(
        postgres
            .execute_many(&insert, vec![arguments(1)])
            .await
            .unwrap(),
        1
    );
    assert!(postgres
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
    let rows: Vec<BatchRow> = postgres
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
            text: "Tom or Jerry ' ? $1".into(),
            bytes: vec![0, 255],
            f: 1.25,
            b: true,
        }]
    );
    assert!(postgres
        .fetch_optional::<(i64,)>(sqlx::query_as(&format!(
            "SELECT id FROM {table} WHERE id = 2"
        )))
        .await
        .unwrap()
        .is_none());
    let mut tx = postgres.begin().await.unwrap();
    sqlx::query_with(&insert, arguments(3))
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let (count,): (i64,) = postgres
        .fetch_one(sqlx::query_as(&format!("SELECT COUNT(*) FROM {table}")))
        .await
        .unwrap();
    assert_eq!(count, 1);
    postgres
        .execute(sqlx::query(&format!("DROP TABLE {table}")))
        .await
        .unwrap();

    assert_eq!(repository.delete(uid).await.unwrap(), 1);
    assert!(repository.find_by_uid(uid).await.unwrap().is_none());
    postgres
        .execute(
            sqlx::query("DELETE FROM group_user WHERE uid IN ($1, $2)")
                .bind(uid)
                .bind(record.uid),
        )
        .await
        .unwrap();
    postgres.pool().close().await;
    std::fs::remove_dir_all(directory).unwrap();
}
