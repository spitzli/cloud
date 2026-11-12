//! Exercises actual Core host/CSRF, actor, Build ceiling and worker routing using the shared fixture.
use super::browser;
use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use tower::ServiceExt;
#[tokio::test]
#[ignore = "owned restricted database fixture"]
async fn mounted_build_routes_preserve_browser_and_worker_authority_boundaries() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let f = fixture.clone();
    let browser_fixture = Arc::new(browser::BrowserFixture::new(&fixture).await);
    let browser = browser_fixture.clone();
    let connection = id(&f.owner).await;
    let worker_id = id(&f.owner).await;
    let cleanup_worker = worker_id.clone();
    let outcome=tokio::spawn(async move{
 f.configure("k3s").await;
 let cookies=CookiePolicy::new(false,"fixture-hosting-cookie-key-at-least-32-bytes").unwrap();let cookie=format!("{}={}",cookies.name(),cookies.sign(&format!("{}-token",f.nonce)).unwrap());let identity=browser.identity.clone();
 let service=Arc::new(webdock_build::BuildService::new(db("DATABASE_URL").await,db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,std::env::var("DATABASE_RLS_SECRET").unwrap(),Arc::new(webdock_persistence::IdAllocator::new(db("DATABASE_URL").await,webdock_ids::Identity::platform(0,2).unwrap())),f.service.clone(),"registry.invalid".into()).await.unwrap().with_session_authority(db("AUTH_DATABASE_URL").await,identity.clone(),Some(browser.browser.clone())).await.unwrap());
 let config=webdock_core::config::PlatformConfig::from_map(&std::collections::HashMap::from([("WEBDOCK_FEATURES".into(),"{\"hosting\":\"enabled\"}".into()),("WEBDOCK_CONSOLE_ORIGIN".into(),"https://console.example.invalid".into()),("WEBDOCK_STUDIO_ORIGIN".into(),"https://studio.example.invalid".into()),("WEBDOCK_AUTH_ORIGIN".into(),"https://auth.example.invalid".into())])).unwrap();
 let router=webdock_control_plane::http::build_app_with_domains(config,db("AUTH_DATABASE_URL").await,webdock_control_plane::http::Services{identity:Some(identity),browser:Some(browser.browser.clone()),hosting:Some(f.service.clone()),build:Some(service.clone()),..Default::default()});
 let post=|body:Value,origin:Option<&str>|{let mut request=Request::builder().method("POST").uri("/v1/hosting").header("host","localhost:3130").header("content-type","application/json").header("cookie",&cookie);if let Some(origin)=origin{request=request.header("origin",origin);}request.body(Body::from(body.to_string())).unwrap()};
 let read=json!({"action":"git.sources.list","projectID":f.pid});assert_eq!(router.clone().oneshot(post(read.clone(),None)).await.unwrap().status(),StatusCode::FORBIDDEN);assert_eq!(router.clone().oneshot(post(read.clone(),Some("http://attacker.invalid"))).await.unwrap().status(),StatusCode::FORBIDDEN);
 let response=router.clone().oneshot(post(read,Some("http://localhost:3130"))).await.unwrap();assert_eq!(response.status(),StatusCode::OK);let bytes=response.into_body().collect().await.unwrap().to_bytes();assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap()["docs"],json!([]));
 let mut draft=spec();draft.as_object_mut().unwrap().remove("image");draft.as_object_mut().unwrap().remove("template");let app=f.service.execute(&f.principal,json!({"action":"apps.prepare","projectID":f.pid,"name":"core-build-target","spec":draft,"subscriptionRevision":1,"idempotencyKey":"core-build-target-1"})).await.unwrap();let app=app["appID"].as_str().or_else(||app["id"].as_str()).unwrap();
 sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state) VALUES($1,$2,'1234','1234','fixture','{}','active')",vec![connection.clone().into(),f.cid.clone().into()]).await;sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'55','{}')",vec![connection.clone().into(),f.cid.clone().into()]).await;
 let configure=json!({"action":"git.source.configure","projectID":f.pid,"targetID":app,"connectionID":connection,"repositoryID":"55","branch":"main","rootDirectory":".","recipe":"dockerfile","revision":0});assert_eq!(service.execute_with_ceiling(&f.principal,configure.clone(),webdock_build::BuildCeiling{read:true,write:false}).await.unwrap_err().code,"permission_denied");assert_eq!(router.clone().oneshot(post(configure,Some("http://localhost:3130"))).await.unwrap().status(),StatusCode::OK);
 let worker_request=|path:&str,credential:Option<&str>|{let mut request=Request::builder().method("POST").uri(path).header("host","localhost:3130").header("content-type","application/json");if let Some(token)=credential{request=request.header("authorization",format!("Bearer {token}"));}request.body(Body::from("{}")).unwrap()};
 assert_eq!(router.clone().oneshot(worker_request("/api/hosting/git/worker/claim",None)).await.unwrap().status(),StatusCode::UNAUTHORIZED);assert_eq!(router.clone().oneshot(worker_request("/api/hosting/git/worker/unrecognized",None)).await.unwrap().status(),StatusCode::FORBIDDEN);
 let credential="S".repeat(43);sql(&f.owner,"INSERT INTO webdock_auth.git_worker(id,credential_hash,enabled,country,isolation,evidence,capacity) VALUES($1,$2,true,'ZZ','microvm','Core fixture',1)",vec![worker_id.clone().into(),hex::encode(Sha256::digest(credential.as_bytes())).into()]).await;let response=router.clone().oneshot(worker_request("/api/hosting/git/worker/claim",Some(&credential))).await.unwrap();assert_eq!(response.status(),StatusCode::OK);assert_eq!(&response.into_body().collect().await.unwrap().to_bytes()[..],b"null");sql(&f.owner,"UPDATE webdock_auth.git_worker SET enabled=false WHERE id=$1",vec![worker_id.into()]).await;assert_eq!(router.clone().oneshot(worker_request("/api/hosting/git/worker/claim",Some(&credential))).await.unwrap().status(),StatusCode::UNAUTHORIZED);
 let browser_read=json!({"action":"git.sources.list","projectID":f.pid});
 let browser_request=|body:Value|Request::builder().method("POST").uri("/api/console/hosting").header("host","console.example.invalid").header("origin","https://console.example.invalid").header("content-type","application/json").header("cookie",&browser.cookie).body(Body::from(body.to_string())).unwrap();
 assert_eq!(router.clone().oneshot(browser_request(browser_read.clone())).await.unwrap().status(),StatusCode::OK);
 service.execute_with_authority(&f.principal,&browser.authority,browser_read.clone(),webdock_build::BuildCeiling{read:true,write:true}).await.unwrap();
 browser.revoke(&f.owner).await;
 assert!(service.execute_with_authority(&f.principal,&browser.authority,browser_read.clone(),webdock_build::BuildCeiling{read:true,write:true}).await.is_err(),"stored browser proof must lose authority independently of its underlying session");
 assert_eq!(router.oneshot(browser_request(browser_read)).await.unwrap().status(),StatusCode::SERVICE_UNAVAILABLE);
 }).await;
    for table in ["git_source", "git_repository", "git_connection"] {
        sql(
            &fixture.owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE customer_id=$1"),
            vec![fixture.cid.clone().into()],
        )
        .await;
    }
    sql(
        &fixture.owner,
        "DELETE FROM webdock_auth.git_worker WHERE id=$1",
        vec![cleanup_worker.into()],
    )
    .await;
    browser_fixture.cleanup(&fixture.owner).await;
    fixture.cleanup().await;
    outcome.unwrap();
}

#[tokio::test]
#[ignore = "owned restricted database fixture; no cluster, registry or worker execution"]
async fn inventory_without_execution_serves_customer_sources_and_closes_all_writes() {
    let fixture = Arc::new(Fixture::new("kubernetes").await);
    let f = fixture.clone();
    let original_browser = Arc::new(browser::BrowserFixture::new(&fixture).await);
    let original = original_browser.clone();
    let connection = id(&f.owner).await;
    let outcome = tokio::spawn(async move {
        f.configure("k3s").await;
        let ids = Arc::new(webdock_persistence::IdAllocator::new(db("AUTH_DATABASE_URL").await, webdock_ids::Identity::platform(0, 2).unwrap()));
        let full = webdock_build::BuildService::new(db("DATABASE_URL").await, db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await, std::env::var("DATABASE_RLS_SECRET").unwrap(), ids.clone(), f.service.clone(), "registry.invalid".into()).await.unwrap()
            .with_session_authority(db("AUTH_DATABASE_URL").await, original.identity.clone(), Some(original.browser.clone())).await.unwrap();
        let mut draft = spec(); draft.as_object_mut().unwrap().remove("image"); draft.as_object_mut().unwrap().remove("template");
        let made = f.service.execute(&f.principal, json!({"action":"apps.prepare","projectID":f.pid,"name":"inventory-target","spec":draft,"subscriptionRevision":1,"idempotencyKey":"inventory-target-1"})).await.unwrap();
        let target = made["appID"].as_str().or_else(|| made["id"].as_str()).unwrap();
        sql(&f.owner,"INSERT INTO webdock_auth.git_connection(id,customer_id,installation_id,account_id,account_login,permissions,state) VALUES($1,$2,'1234','1234','fixture','{}','active')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'55','{\"fullName\":\"fixture/connected\"}')",vec![connection.clone().into(),f.cid.clone().into()]).await;
        let configure = json!({"action":"git.source.configure","projectID":f.pid,"targetID":target,"connectionID":connection,"repositoryID":"55","branch":"main","rootDirectory":".","recipe":"dockerfile","revision":0});
        let source = full.execute(&f.principal, configure.clone()).await.unwrap();
        let source_id = source["id"].as_str().unwrap();
        let operator_inventory = webdock_build::BuildService::new_inventory(db("DATABASE_URL").await,db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,std::env::var("DATABASE_RLS_SECRET").unwrap(),ids.clone(),f.service.clone()).await.unwrap();
        assert_eq!(operator_inventory.enroll_worker(&f.principal,"DE","artifact-only","Owned fixture; execution must stay disabled",1).await.unwrap_err().code,"build_execution_unavailable");
        assert_eq!(operator_inventory.configure_kubernetes_admission(&f.principal,true,1,1).await.unwrap_err().code,"build_execution_unavailable");
        // Use a real customer membership and re-authenticated canonical principal, not a forged role.
        sql(&f.owner,"UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",vec![f.uid.clone().into()]).await;
        sql(&f.owner,"INSERT INTO webdock_auth.member(id,\"organizationId\",\"userId\",role,\"createdAt\") SELECT $1,t.organization_id,$2,'owner',now() FROM webdock_auth.tenant_customer t WHERE t.customer_id=$3",vec![format!("{}-member",f.nonce).into(),f.uid.clone().into(),f.cid.clone().into()]).await;
        let customer_browser = browser::BrowserFixture::new(&f).await;
        let cookies = CookiePolicy::new(false,"fixture-hosting-cookie-key-at-least-32-bytes").unwrap();
        let customer_cookie = format!("{}={}",cookies.name(),cookies.sign(&format!("{}-token",f.nonce)).unwrap());
        let customer = customer_browser.identity.authenticate_cookie(&customer_cookie).await.unwrap().unwrap();
        assert!(!customer.is_operator());
        let inventory = Arc::new(webdock_build::BuildService::new_inventory(db("DATABASE_URL").await,db("WEBDOCK_PLUGIN_WORKER_DATABASE_URL").await,std::env::var("DATABASE_RLS_SECRET").unwrap(),ids,f.service.clone()).await.unwrap()
            .with_session_authority(db("AUTH_DATABASE_URL").await,customer_browser.identity.clone(),Some(customer_browser.browser.clone())).await.unwrap());
        assert!(!inventory.execution_configured());
        let config = webdock_core::config::PlatformConfig::from_map(&std::collections::HashMap::from([
            ("WEBDOCK_FEATURES".into(), "{\"hosting\":\"enabled\"}".into()),
            ("WEBDOCK_CONSOLE_ORIGIN".into(), "https://console.example.invalid".into()),
            ("WEBDOCK_AUTH_ORIGIN".into(), "https://auth.example.invalid".into()),
            ("WEBDOCK_STUDIO_ORIGIN".into(), "https://studio.example.invalid".into()),
        ])).unwrap();
        let router = webdock_control_plane::http::build_app_with_domains(config, db("AUTH_DATABASE_URL").await, webdock_control_plane::http::Services {
            identity: Some(customer_browser.identity.clone()), browser: Some(customer_browser.browser.clone()), hosting: Some(f.service.clone()), build: Some(inventory.clone()), ..Default::default()
        });
        let request = |input:Value| Request::builder().method("POST").uri("/api/console/hosting").header("host","console.example.invalid").header("origin","https://console.example.invalid").header("content-type","application/json").header("cookie",&customer_browser.cookie).body(Body::from(input.to_string())).unwrap();
        let response = router.oneshot(request(json!({"action":"git.sources.list","projectID":f.pid}))).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap()["docs"][0]["repositoryName"], "fixture/connected");

        let read = json!({"action":"git.sources.list","projectID":f.pid});
        let ceiling = || webdock_build::BuildCeiling { read: true, write: true };
        let listed = inventory.execute_with_authority(&customer,&customer_browser.authority,read.clone(),ceiling()).await.unwrap();
        assert_eq!(listed["docs"][0]["repositoryName"],"fixture/connected");
        let info = inventory.execute_with_authority(&customer,&customer_browser.authority,json!({"action":"git.source.get","projectID":f.pid}),ceiling()).await.unwrap();
        for field in ["canBuild","canConfigure","canApprove"] { assert_eq!(info[field],false); }
        assert!(info["blockers"].as_array().unwrap().iter().any(|v|v.as_str().unwrap().contains("read-only")));
        for command in [configure,json!({"action":"git.builds.request","projectID":f.pid,"targetID":target,"idempotencyKey":"inventory-build-1"}),json!({"action":"git.source.webhook.configure","projectID":f.pid,"sourceID":source_id,"revision":1,"enabled":true}),json!({"action":"git.connections.disconnect","customerID":f.cid,"connectionID":connection,"revision":1})] {
            assert_eq!(inventory.execute_with_authority(&customer,&customer_browser.authority,command,ceiling()).await.unwrap_err().code,"build_execution_unavailable");
        }
        assert!(inventory.execute_with_authority(&customer,&customer_browser.authority,json!({"action":"git.sources.list","projectID":"9223372036854775806"}),ceiling()).await.is_err());
        assert_eq!(inventory.authenticate_worker(&"S".repeat(43)).await.err().unwrap().code,"build_execution_unavailable");
        assert_eq!(inventory.receive_github_webhook(b"{}","","","push",true).await.unwrap_err().code,"build_execution_unavailable");
        let response = webdock_build::http::worker_router(inventory.clone()).oneshot(Request::builder().method("POST").uri("/claim").body(Body::from("{}")).unwrap()).await.unwrap();
        assert_eq!(response.status(),StatusCode::NOT_FOUND);
        for action in ["git.builds.list", "git.releases.list"] {
            let history = inventory.execute_with_authority(&customer,&customer_browser.authority,json!({"action":action,"projectID":f.pid}),ceiling()).await.unwrap();
            assert!(history["docs"].as_array().unwrap().is_empty());
        }
        let unchanged = inventory.execute_with_authority(&customer,&customer_browser.authority,read.clone(),ceiling()).await.unwrap(); assert_eq!(listed,unchanged);
        customer_browser.revoke(&f.owner).await;
        assert!(inventory.execute_with_authority(&customer,&customer_browser.authority,read,ceiling()).await.is_err());
        customer_browser.cleanup(&f.owner).await;
    }).await;
    for table in ["git_source", "git_repository", "git_connection"] {
        sql(
            &fixture.owner,
            &format!("DELETE FROM webdock_auth.{table} WHERE customer_id=$1"),
            vec![fixture.cid.clone().into()],
        )
        .await;
    }
    original_browser.cleanup(&fixture.owner).await;
    fixture.cleanup().await;
    outcome.unwrap();
}
