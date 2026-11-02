#![cfg(feature = "test-support")]
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use webdock_mail::jmap::{EmailChange, EmailQuery, JmapClient, MailAddress, MailDraft};
async fn client(
    handler: impl Fn(Value) -> Value + Send + Sync + 'static,
) -> (JmapClient, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let handler = Arc::new(handler);
    let router = Router::new().route(
        "/jmap",
        post(
            move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
                let handler = handler.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer local-fixture");
                    Json(handler(body))
                }
            },
        ),
    );
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (
        JmapClient::local_fixture(
            &url,
            "Bearer local-fixture",
            "account-a",
            Some(MailAddress {
                email: "alice@example.test".into(),
                name: Some("Support".into()),
            }),
        )
        .unwrap(),
        task,
    )
}
fn reply(body: &Value, result: Value) -> Value {
    let call = &body["methodCalls"][0];
    json!({"methodResponses":[[call[0],result,call[2]]]})
}
#[tokio::test]
async fn account_and_message_binding_bounded_queries_and_uncertain_mutations() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let captured = calls.clone();
    let (c,task)=client(move|body|{let call=&body["methodCalls"][0];assert_eq!(call[1]["accountId"],"account-a");captured.lock().unwrap().push(call.clone());
        let result=match call[0].as_str().unwrap(){"Mailbox/get"=>json!({"accountId":"account-b","list":[]}),"Email/query"=>json!({"accountId":"account-a","ids":["email1"],"total":1,"queryState":"q1"}),"Email/get"=>json!({"accountId":"account-a","list":[{"id":"foreign"}]}),_=>json!({"accountId":"account-a","notUpdated":{"message1":{"type":"forbidden","description":"PRIVATE"}}})};reply(&body,result)}).await;
    assert!(c.mailboxes().await.is_err());
    assert!(
        c.emails(EmailQuery {
            limit: 101,
            ..Default::default()
        })
        .await
        .is_err()
    );
    assert_eq!(calls.lock().unwrap().len(), 1);
    assert!(
        c.emails(EmailQuery {
            mailbox_id: Some("inbox".into()),
            text: Some("invoice".into()),
            ..Default::default()
        })
        .await
        .is_err()
    );
    assert_eq!(
        calls.lock().unwrap()[1][1]["filter"],
        json!({"inMailbox":"inbox","text":"invoice"})
    );
    let error = c
        .update_email(
            "message1",
            EmailChange {
                seen: Some(true),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(!error.uncertain);
    assert!(!error.to_string().contains("PRIVATE"));
    task.abort();
    let calls = Arc::new(Mutex::new(0));
    let captured = calls.clone();
    let (c, task) = client(move |_| {
        *captured.lock().unwrap() += 1;
        json!({"invalid":"PRIVATE"})
    })
    .await;
    assert!(
        c.update_email(
            "message1",
            EmailChange {
                seen: Some(true),
                ..Default::default()
            },
            None
        )
        .await
        .unwrap_err()
        .uncertain
    );
    assert_eq!(*calls.lock().unwrap(), 1);
    task.abort();
}
#[tokio::test]
async fn replacement_keeps_old_draft_until_new_is_saved_and_fences_cleanup() {
    let changes = Arc::new(Mutex::new(Vec::new()));
    let captured = changes.clone();
    let(c,task)=client(move |body|{let call=&body["methodCalls"][0];let args=&call[1];let mut result=match call[0].as_str().unwrap(){
        "Identity/get"=>json!({"list":[{"id":"sender","email":"alice@example.test","name":"Alice"}]}),
        "Mailbox/get"=>json!({"list":[{"id":"drafts","role":"drafts"}]}),
        "Email/get"=>json!({"state":"v1","list":[{"id":"old","from":[{"email":"alice@example.test"}],"keywords":{"$draft":true}}]}),
        "Email/set"=>{captured.lock().unwrap().push(args.clone());if args.get("destroy").is_some(){json!({"destroyed":["old"],"newState":"v3"})}else{json!({"created":{"draft":{"id":"new"}},"newState":"v2"})}},_=>panic!("unexpected method")};result["accountId"]=json!("account-a");reply(&body,result)}).await;
    let draft = MailDraft {
        identity_id: "sender".into(),
        to: vec![],
        cc: vec![],
        bcc: vec![],
        subject: "Working draft".into(),
        text: "Edited".into(),
        in_reply_to: None,
        references: None,
        attachments: vec![],
    };
    let result = c.create_draft(draft, Some("old")).await.unwrap();
    assert_eq!(result["id"], "new");
    let changes = changes.lock().unwrap();
    assert!(changes[0].get("destroy").is_none());
    assert_eq!(changes[0]["ifInState"], "v1");
    assert_eq!(
        changes[0]["create"]["draft"]["from"],
        json!([{"email":"alice@example.test","name":"Support"}])
    );
    assert_eq!(changes[1]["ifInState"], "v2");
    assert_eq!(changes[1]["destroy"], json!(["old"]));
    task.abort();
}

#[tokio::test]
async fn rejected_draft_keeps_original_and_malformed_submission_never_becomes_success() {
    let changes = Arc::new(Mutex::new(Vec::new()));
    let captured = changes.clone();
    let(c,task)=client(move|body|{let call=&body["methodCalls"][0];let args=&call[1];captured.lock().unwrap().push(call[0].as_str().unwrap().to_owned());
        assert!(args.get("destroy").is_none());
        let mut result=match call[0].as_str().unwrap(){
            "Identity/get"=>json!({"list":[{"id":"sender","email":"alice@example.test"}]}),
            "Mailbox/get"=>json!({"list":[{"id":"drafts","role":"drafts"},{"id":"sent","role":"sent"}]}),
            "Email/get"=>json!({"state":"v1","list":[{"id":"old","from":[{"email":"alice@example.test"}],"keywords":{"$draft":true}}]}),
            "Email/set"=>if args.get("update").is_some(){assert_eq!(args["update"]["old"],json!({"keywords/$draft":null,"mailboxIds":{"sent":true}}));json!({"updated":{"old":null}})}else{json!({"notCreated":{"draft":{"type":"overQuota"}}})},
            "EmailSubmission/set"=>json!({"created":{"submission":{"id":"invalid/id"}}}),_=>panic!("unexpected method")};result["accountId"]=json!("account-a");reply(&body,result)}).await;
    let draft = MailDraft {
        identity_id: "sender".into(),
        to: vec![],
        cc: vec![],
        bcc: vec![],
        subject: "Update".into(),
        text: "Keep original".into(),
        in_reply_to: None,
        references: None,
        attachments: vec![],
    };
    assert!(
        !c.create_draft(draft, Some("old"))
            .await
            .unwrap_err()
            .uncertain
    );
    assert!(c.submit_draft("old", "sender").await.unwrap_err().uncertain);
    assert_eq!(
        changes
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.as_str() == "EmailSubmission/set")
            .count(),
        1
    );
    changes.lock().unwrap().clear();
    c.file_sent("old").await.unwrap();
    assert_eq!(*changes.lock().unwrap(), ["Mailbox/get", "Email/set"]);
    task.abort();
}

#[tokio::test]
async fn binary_transfers_bind_account_message_and_size_and_export_bcc_as_envelope() {
    use axum::{extract::Path, routing::get};
    let downloads = Arc::new(Mutex::new(Vec::new()));
    let captured = downloads.clone();
    let router=Router::new().route("/jmap",post(|Json(body):Json<Value>|async move{
        let call=&body["methodCalls"][0];assert_eq!(call[1]["accountId"],"account-a");
        let mut result=if call[0]=="Identity/get"{json!({"list":[{"id":"sender","email":"alice@example.test"}]})}else{json!({"list":[{"id":"email1","blobId":"message","size":100,"keywords":{"$draft":true},"from":[{"email":"alice@example.test"}],"to":[{"email":"bob@example.test","name":null}],"bcc":[{"email":"hidden@example.test","name":null}],"attachments":[{"blobId":"blob1","name":"note.txt","type":"text/plain","size":5}]}]})};result["accountId"]=json!("account-a");Json(reply(&body,result))
    })).route("/jmap/upload/{account}/",post(|Path(account):Path<String>,bytes:axum::body::Bytes|async move{assert_eq!(account,"account-a");assert_eq!(bytes,"hello");Json(json!({"accountId":account,"blobId":"blob1","type":"text/plain","size":5}))}))
    .route("/jmap/download/{account}/{blob}/{name}",get(move |Path((account,blob,_)):Path<(String,String,String)>|{let captured=captured.clone();async move{assert_eq!(account,"account-a");captured.lock().unwrap().push(blob.clone());if blob=="message"{"From: alice@example.test\r\nBcc: hidden@example.test\r\n\r\nBody"}else{"hello"}}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let c = JmapClient::local_fixture(&origin, "Bearer fixture", "account-a", None).unwrap();
    assert_eq!(
        c.upload(b"hello".to_vec(), "text/plain").await.unwrap()["blobId"],
        "blob1"
    );
    assert!(c.download_attachment("email1", "foreign").await.is_err());
    assert!(downloads.lock().unwrap().is_empty());
    let file = c.download_attachment("email1", "blob1").await.unwrap();
    assert_eq!(file.bytes, b"hello");
    assert_eq!(file.name, "note.txt");
    let exported = c.export_draft("email1", "sender").await.unwrap();
    assert_eq!(exported.to, ["bob@example.test", "hidden@example.test"]);
    assert_eq!(exported.sender.email, "alice@example.test");
    assert!(exported.raw.ends_with(b"Body"));
    assert_eq!(*downloads.lock().unwrap(), ["blob1", "message"]);
    task.abort();
}
