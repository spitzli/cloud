//! Live HTTP ordering with only ordinary owned customer-row barriers. No production hooks.
use super::*;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug)]
enum Action {
    Read,
    Write,
    Replay,
}
impl Action {
    fn method(self) -> &'static str {
        if matches!(self, Self::Read) {
            "GET"
        } else {
            "POST"
        }
    }
    fn body(self, revision: i32, key: &str) -> Value {
        if matches!(self, Self::Read) {
            Value::Null
        } else if matches!(self, Self::Replay) {
            command(0, "preview-race-initial")
        } else {
            command(revision, key)
        }
    }
}
async fn customer(c: &Context<'_>, name: &str) -> String {
    let reply = request(
        c.app,
        CONSOLE,
        "POST",
        "/api/console/registry/customers",
        c.console,
        json!({"name":name}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);
    reply.body["id"].as_str().unwrap().to_owned()
}
async fn barrier(c: &Context<'_>, customer: &str) -> (sea_orm::DatabaseTransaction, i32) {
    let held = c.owner.begin().await.unwrap();
    let pid = held
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid FROM webdock_admin.customers WHERE id=$1 FOR UPDATE",
            [customer.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    (held, pid)
}
async fn blocker(c: &Context<'_>, blocked_by: i32, query: &str) -> Option<i32> {
    c.business.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "SELECT pid FROM pg_stat_activity WHERE datname=current_database() AND usename=current_user AND pid<>pg_backend_pid() AND $1=ANY(pg_blocking_pids(pid)) AND query LIKE $2 ORDER BY pid LIMIT 1",
        [blocked_by.into(),query.into()])).await.unwrap().map(|row|row.try_get("","pid").unwrap())
}
async fn observe_blocked(c: &Context<'_>, blocked_by: i32, query: &str) -> i32 {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(pid) = blocker(c, blocked_by, query).await {
                return pid;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("ordinary customer barrier must block the actual business transaction")
}
fn allowance(c: &Context<'_>, path: &str, action: Action, body: Value) -> JoinHandle<Reply> {
    let app = c.app.clone();
    let path = path.to_owned();
    let session = c.console.to_owned();
    tokio::spawn(
        async move { request(&app, CONSOLE, action.method(), &path, &session, body).await },
    )
}
fn preview(c: &Context<'_>, customer: &str, mode: &str) -> JoinHandle<Reply> {
    let app = c.app.clone();
    let session = c.console.to_owned();
    let body = json!({"customerID":customer,"mode":mode});
    tokio::spawn(async move {
        request(
            &app,
            CONSOLE,
            "POST",
            "/api/console/tenant-preview/start",
            &session,
            body,
        )
        .await
    })
}
async fn finish(handle: JoinHandle<Reply>) -> Reply {
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .unwrap()
        .unwrap()
}
async fn waits_for(c: &Context<'_>, handle: &JoinHandle<Reply>, first: i32) -> bool {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handle.is_finished() {
                return false;
            }
            if blocker(c, first, "%pg_advisory_xact_lock%").await.is_some() {
                return true;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("contender must either finish or observably wait on first transaction")
}
async fn exit(c: &Context<'_>) {
    let reply = request(
        c.app,
        CONSOLE,
        "POST",
        "/api/console/tenant-preview/exit",
        c.console,
        json!({}),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "normal public preview exit");
}
async fn preview_first(
    c: &Context<'_>,
    a: &str,
    b: &str,
    path: &str,
    mode: &str,
    action: Action,
    revision: i32,
) {
    let (held, pid) = barrier(c, b).await;
    let preview = preview(c, b, mode);
    let first = observe_blocked(c, pid, "%SELECT c.name,t.organization_id%FOR SHARE OF c%").await;
    let operation = allowance(
        c,
        path,
        action,
        action.body(revision, &format!("preview-first-{mode}-{action:?}")),
    );
    let serialized = waits_for(c, &operation, first).await;
    held.commit().await.unwrap();
    let operation = finish(operation).await;
    let preview = finish(preview).await;
    assert_eq!(preview.status, StatusCode::OK);
    let unchanged = saved(c, a).await["revision"] == revision;
    exit(c).await;
    eprintln!(
        "allowance_preview_order order=preview-first mode={mode} action={action:?} serialized={serialized} allowance_status={} revision_unchanged={unchanged}",
        operation.status
    );
    assert!(
        serialized,
        "preview-first allowance must wait for the real same-session preview transaction"
    );
    assert!(matches!(
        operation.status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert!(unchanged);
}
async fn allowance_first(
    c: &Context<'_>,
    a: &str,
    b: &str,
    path: &str,
    mode: &str,
    action: Action,
    revision: i32,
) -> i32 {
    let (held, pid) = barrier(c, a).await;
    let operation = allowance(
        c,
        path,
        action,
        action.body(revision, &format!("allowance-first-{mode}-{action:?}")),
    );
    let first = observe_blocked(c, pid, "%FOR UPDATE OF c FOR SHARE OF t%").await;
    let preview = preview(c, b, mode);
    let serialized = waits_for(c, &preview, first).await;
    held.commit().await.unwrap();
    let operation = finish(operation).await;
    let preview = finish(preview).await;
    assert_eq!(preview.status, StatusCode::OK);
    // Current scope denies all three boundaries while B's actual preview is active.
    let current = request(
        c.app,
        CONSOLE,
        action.method(),
        path,
        c.console,
        action.body(revision, "during-preview"),
    )
    .await;
    exit(c).await;
    eprintln!(
        "allowance_preview_order order=allowance-first mode={mode} action={action:?} serialized={serialized} allowance_status={}",
        operation.status
    );
    assert!(
        serialized,
        "preview B must wait for the in-flight same-session allowance A transaction"
    );
    assert_eq!(operation.status, StatusCode::OK);
    assert!(matches!(
        current.status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    let expected = if matches!(action, Action::Write) {
        revision + 1
    } else {
        revision
    };
    assert_eq!(saved(c, a).await["revision"], expected);
    expected
}
pub(super) async fn check(c: &Context<'_>) {
    assert_eq!(
        activate(c.app, c.console, HOSTING, "preview-race-hosting")
            .await
            .status,
        StatusCode::OK
    );
    let a = customer(c, "Native preview race allowance customer A").await;
    let b = customer(c, "Native preview race separate preview customer B").await;
    let native = c
        .identity
        .authenticate_cookie(c.native)
        .await
        .unwrap()
        .unwrap();
    let browser = c
        .browser
        .resolve_authority(CONSOLE, c.console)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(native.session_id, browser.principal.session_id);
    let path = format!("/api/console/access/customers/{a}/allowances");
    let initial = configured(c, &path, command(0, "preview-race-initial")).await;
    assert_eq!(initial.status, StatusCode::OK);
    let mut revision = 1;
    for mode in ["read_only", "owner"] {
        for action in [Action::Write, Action::Replay, Action::Read] {
            // Allowance-first is first so old source exposes actual preview overtaking.
            revision = allowance_first(c, &a, &b, &path, mode, action, revision).await;
            preview_first(c, &a, &b, &path, mode, action, revision).await;
        }
    }
    if std::env::var("WEBDOCK_ALLOWANCE_BUILTIN_FENCE_TEST").as_deref() == Ok("1") {
        builtin_participates(c, &b, &native.session_id).await;
    }
    retained_configuration_failure(c, &a, &path, &native.session_id, revision).await;
    let fresh = console_session(c).await;
    assert_eq!(
        request(
            c.app,
            CONSOLE,
            "POST",
            &path,
            &fresh,
            command(0, "preview-race-initial")
        )
        .await
        .body,
        initial.body
    );
    eprintln!(
        "native_allowance_preview_races_complete actual_customers={a},{b} same_native_session=true cases=12 revision={revision}"
    );
}

async fn session_barrier(c: &Context<'_>, session: &str) -> (sea_orm::DatabaseTransaction, i32) {
    let held = c.owner.begin().await.unwrap();
    let pid = held
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            "SELECT pg_backend_pid() AS pid",
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "pid")
        .unwrap();
    held.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
        [format!("native-session-context:{session}").into()],
    ))
    .await
    .unwrap();
    (held, pid)
}
async fn builtin_participates(c: &Context<'_>, b: &str, session: &str) {
    // Separately gated inherited Host boundary; this test never changes Host production.
    let (held, pid) = session_barrier(c, session).await;
    let preview = preview(c, b, "read_only");
    let preview_pid = observe_blocked(c, pid, "%pg_advisory_xact_lock%").await;
    let app = c.app.clone();
    let cookie = c.console.to_owned();
    let operation =
        tokio::spawn(async move { activate(&app, &cookie, HOSTING, "preview-race-hosting").await });
    // Both queue behind the same owner barrier. A nonparticipant completes instead.
    let serialized = waits_for(c, &operation, preview_pid).await;
    held.commit().await.unwrap();
    let operation = finish(operation).await;
    let preview = finish(preview).await;
    assert_eq!(preview.status, StatusCode::OK);
    exit(c).await;
    eprintln!(
        "builtin_preview_order action=retained-hosting-activation preview_first=true serialized={serialized} builtin_status={}",
        operation.status
    );
    assert!(
        serialized,
        "builtin retained replay must participate in the same native session fence"
    );
    assert!(matches!(
        operation.status,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
}
async fn retained_configuration_failure(
    c: &Context<'_>,
    a: &str,
    path: &str,
    session: &str,
    revision: i32,
) {
    // This is an adverse canonical configuration change after initial real browser
    // resolution, not a fake proof and not an injected/aborted auth transaction.
    let before = saved(c, a).await;
    let (held, pid) = session_barrier(c, session).await;
    let operation = allowance(
        c,
        path,
        Action::Write,
        command(revision, "retained-config-fault"),
    );
    observe_blocked(c, pid, "%pg_advisory_xact_lock%").await;
    held.execute_raw(Statement::from_sql_and_values(DbBackend::Postgres,
        "UPDATE webdock_auth.\"oauthClient\" SET \"clientSecret\"=repeat('0',43) WHERE \"clientId\"=$1",
        [c.secrets["clientID"].as_str().unwrap().into()])).await.unwrap();
    held.commit().await.unwrap();
    let operation = finish(operation).await;
    assert_eq!(operation.status, StatusCode::SERVICE_UNAVAILABLE);
    sql(
        c.owner,
        "UPDATE webdock_auth.\"oauthClient\" SET \"clientSecret\"=$2 WHERE \"clientId\"=$1",
        vec![
            c.secrets["clientID"].as_str().unwrap().into(),
            webdock_oauth::legacy_token_hash(
                c.secrets["environment"]["WEBDOCK_SSO_CLIENT_SECRET"]
                    .as_str()
                    .unwrap(),
            )
            .into(),
        ],
    )
    .await;
    assert_eq!(saved(c, a).await, before);
    eprintln!(
        "native_allowance_retained_config_failure status=503 source_unchanged=true real_browser_proof=true"
    );
}
