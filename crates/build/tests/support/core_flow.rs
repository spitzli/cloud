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
