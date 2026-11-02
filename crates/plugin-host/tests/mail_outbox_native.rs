//! Actual managed Mail acceptance. All infrastructure cases require explicit opt-in AND --ignored.
//! Core is fixture-hosted production code; Mail/gateway are actual separate executables.
#[path = "support/mail_outbox_catcher.rs"]
#[allow(
    dead_code,
    reason = "Reusable raw-wire equality and consuming-shutdown APIs are tested in mail_outbox_catcher"
)]
mod catcher;
#[path = "support/mail_outbox_native_fixture.rs"]
mod fixture;

use fixture::{AttemptTotals, Fault, Fixture, Revocation, Source, TestResult};

#[tokio::test]
async fn execution_fence_witness_waits_for_positive_blocker_without_masking_errors() {
    let mut request = std::pin::pin!(std::future::pending::<TestResult<()>>());
    let calls = std::cell::Cell::new(0);
    let seen = fixture::wait_for_fence_witness(
        request.as_mut(),
        async || {
            calls.set(calls.get() + 1);
            Ok(calls.get() >= 3)
        },
        std::time::Duration::from_millis(100),
    )
    .await;
    assert!(
        seen.is_ok() && calls.get() >= 3,
        "single blind timing sample missed delayed fence arrival"
    );
    let mut early = std::pin::pin!(std::future::ready(Ok::<(), &'static str>(())));
    assert!(
        fixture::wait_for_fence_witness(
            early.as_mut(),
            async || Ok(true),
            std::time::Duration::from_millis(100)
        )
        .await
        .is_err(),
        "completed request passed held-fence proof"
    );
    let mut request = std::pin::pin!(std::future::pending::<TestResult<()>>());
    assert!(
        fixture::wait_for_fence_witness(
            request.as_mut(),
            async || Err("owned probe unavailable"),
            std::time::Duration::from_millis(100)
        )
        .await
        .is_err(),
        "probe error passed held-fence proof"
    );
    assert!(
        fixture::wait_for_fence_witness(
            request.as_mut(),
            async || Ok(false),
            std::time::Duration::from_millis(15)
        )
        .await
        .is_err(),
        "missing witness passed held-fence proof"
    );
}

#[tokio::test]
async fn execution_native_hash_mismatch_preserves_production_bad_request_status() {
    use webdock_mail::send_control::{ControlError, SendControl};
    struct Mismatch;
    #[async_trait::async_trait]
    impl SendControl for Mismatch {
        async fn call(&self, input: serde_json::Value) -> Result<serde_json::Value, ControlError> {
            match input["action"].as_str() {
                Some("send.authenticate") => Ok(serde_json::json!({"customerID":"123"})),
                Some("send.admit") => Err(ControlError::invalid()),
                _ => Err(ControlError::unavailable()),
            }
        }
    }
    let dir = tempfile::tempdir().unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let store = webdock_mail::payload::PrivatePayloadStore::open(
        dir.path(),
        &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [17; 32]),
    )
    .unwrap();
    let api = webdock_mail::send_api::SendApi::new(
        std::sync::Arc::new(Mismatch),
        std::sync::Arc::new(store),
    );
    let request=axum::http::Request::builder().method("POST").uri("/v1/emails").header("authorization",format!("Bearer wd_mail_{}","t".repeat(43))).header("content-type","application/json").header("idempotency-key","same-operation").body(axum::body::Body::from(r#"{"from":"sender@native.example.test","to":"target@example.test","subject":"fixture","text":"fixture"}"#)).unwrap();
    use tower::ServiceExt;
    let response = api.router().oneshot(request).await.unwrap();
    assert!(
        response.status().as_u16() == Source::Native.changed_input_status(),
        "fixture hash-conflict status disagrees with actual native router mapping"
    );
    assert!(
        Source::Action.changed_input_status() == 409,
        "plugin operation conflict status changed"
    );
}

#[tokio::test]
async fn execution_core_origins_keep_production_configuration_and_host_boundaries() {
    use tower::ServiceExt;
    let auth = "https://localhost:34001";
    let console = "https://localhost:34002";
    let config = fixture::fixture_core_configuration(auth, console);
    assert!(
        config.is_ok(),
        "owned fixture duplicated canonical Auth/Console origins"
    );
    let config = config.unwrap();
    assert!(
        config.auth_origin == auth && config.console_origin() == console,
        "fixture profile mapping changed"
    );
    let app = webdock_control_plane::http::build_app_with_domains(
        config,
        sea_orm::DatabaseConnection::default(),
        webdock_control_plane::http::Services::default(),
    );
    for (host, path, expected) in [
        ("localhost:34002", "/api/console/plugins/invoke", 503),
        ("localhost:34001", "/api/console/plugins/invoke", 404),
        ("localhost:34001", "/api/mail/gateway", 503),
        ("localhost:34002", "/api/mail/gateway", 404),
    ] {
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("host", host)
                    .header(
                        "origin",
                        if host == "localhost:34002" {
                            console
                        } else {
                            auth
                        },
                    )
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            response.status().as_u16() == expected,
            "production profile routing changed or was bypassed"
        );
    }
}

#[test]
fn execution_failure_preserves_original_custody() {
    assert!(
        !fixture::cleanup_after_case(&Ok(Err("owned fixture failure"))),
        "failed case would delete original PG/custody"
    );
    assert!(
        !fixture::cleanup_after_case(&Err(Box::new("owned panic"))),
        "panicked case would delete original PG/custody"
    );
    assert!(
        fixture::cleanup_after_case(&Ok(Ok(()))),
        "successful case would skip verified cleanup"
    );
}

#[test]
fn execution_platform_id_obeys_actual_native_constraint() {
    let run = "d4506ba2736b4e22a794a5fa4508c54e";
    let id = fixture::fixture_platform_id(run).unwrap();
    assert!(
        id.len() <= 24
            && id.as_bytes()[0].is_ascii_lowercase()
            && id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'),
        "fixture platform ID violates native/catalog bound"
    );
    assert!(
        id == "task4-d4506ba2736b4e22a7",
        "fixture platform ID derivation changed unexpectedly"
    );
    assert!(
        fixture::fixture_platform_id("not-a-run-id").is_err(),
        "invalid run identity accepted"
    );
}

#[test]
fn fix1_transition_domain_targets_actual_sender_domain() {
    let sender = Source::Native.sender_address();
    assert!(
        sender.split_once('@').map(|(_, domain)| domain)
            == Some(Source::Native.transition_domain()),
        "native deletion barrier targeted unrelated domain"
    );
    assert!(
        Source::Action.transition_domain() == "action.example.test",
        "existing action domain changed"
    );
}

#[tokio::test]
async fn fix1_attempt_window_detects_duplicates_before_entry_and_exact_mode_totals() {
    use catcher::Counts;
    use fixture::{AttemptTotals, observe_attempts};
    let one = Counts {
        data_attempts: 1,
        complete_data: 1,
        stored: 1,
        ..Counts::default()
    };
    let duplicate = Counts {
        data_attempts: 2,
        complete_data: 2,
        stored: 2,
        ..Counts::default()
    };
    assert!(
        observe_attempts(
            one,
            AttemptTotals::ONE_STORED,
            || duplicate,
            std::future::ready(())
        )
        .await
        .is_err(),
        "duplicate during restart/replay escaped saved baseline"
    );
    assert!(
        observe_attempts(
            duplicate,
            AttemptTotals::ONE_STORED,
            || duplicate,
            std::future::ready(())
        )
        .await
        .is_err(),
        "late duplicate baseline escaped absolute total"
    );
    assert!(
        observe_attempts(
            Counts::default(),
            AttemptTotals::NONE,
            || Counts {
                data_attempts: 1,
                ..Counts::default()
            },
            std::future::ready(())
        )
        .await
        .is_err(),
        "lost begin allowed a DATA attempt"
    );
    let rejected = Counts {
        data_attempts: 1,
        complete_data: 1,
        ..Counts::default()
    };
    assert!(
        observe_attempts(
            rejected,
            AttemptTotals::rejected(catcher::Mode::RejectAfterData).unwrap(),
            || Counts {
                data_attempts: 2,
                complete_data: 2,
                ..Counts::default()
            },
            std::future::ready(())
        )
        .await
        .is_err(),
        "rejected duplicate hidden by zero stored count"
    );
    assert!(
        observe_attempts(
            one,
            AttemptTotals::ONE_STORED,
            || Counts {
                complete_data: 2,
                ..one
            },
            std::future::ready(())
        )
        .await
        .is_err(),
        "complete DATA changes ignored"
    );
    assert!(
        observe_attempts(
            one,
            AttemptTotals::ONE_STORED,
            || Counts {
                active: 3,
                blocked: 1,
                ..one
            },
            std::future::ready(())
        )
        .await
        .is_ok(),
        "live connection gauges treated as message duplicates"
    );
    assert!(
        observe_attempts(
            rejected,
            AttemptTotals::rejected(catcher::Mode::RejectAfterData).unwrap(),
            || rejected,
            std::future::ready(())
        )
        .await
        .is_ok(),
        "rejected complete DATA counted as stored"
    );
    let live = std::cell::Cell::new(one);
    assert!(
        observe_attempts(one, AttemptTotals::ONE_STORED, || live.get(), async {
            live.set(duplicate)
        })
        .await
        .is_err(),
        "duplicate during observation escaped original baseline"
    );
    assert!(
        observe_attempts(
            Counts::default(),
            AttemptTotals::NONE,
            Counts::default,
            std::future::ready(())
        )
        .await
        .is_ok(),
        "zero-attempt lost-begin expectation rejected"
    );
}

#[test]
fn fix1_typed_denial_rejects_unavailability_and_wrong_error_class() {
    use fixture::require_control_denial;
    use webdock_mail_protocol::native_control::ControlError;
    assert!(
        require_control_denial(Err(ControlError { status: 503 }), 403).is_err(),
        "database/timeout failure passed authorization proof"
    );
    assert!(
        require_control_denial(Err(ControlError { status: 400 }), 403).is_err(),
        "invalid input passed authorization proof"
    );
    assert!(
        require_control_denial(Err(ControlError { status: 403 }), 403).is_ok(),
        "actual permission denial rejected"
    );
    assert!(
        require_control_denial(Err(ControlError { status: 400 }), 400).is_ok(),
        "explicit malformed-input expectation changed"
    );
    assert!(
        require_control_denial(Err(ControlError { status: 503 }), 400).is_err(),
        "unavailability passed malformed-input proof"
    );
    assert!(
        require_control_denial(Ok(serde_json::Value::Null), 403).is_err(),
        "success passed denial proof"
    );
    assert!(
        !fixture::certificate_rejected(&std::io::Error::from(
            std::io::ErrorKind::ConnectionRefused
        )),
        "TCP refusal passed wrong-CA proof"
    );
    assert!(
        fixture::certificate_rejected(&std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            tokio_rustls::rustls::Error::InvalidCertificate(
                tokio_rustls::rustls::CertificateError::UnknownIssuer
            )
        )),
        "typed certificate rejection was lost"
    );
}

#[tokio::test]
async fn fix1_stopped_child_journal_failure_keeps_slot_and_sticky_private_custody() {
    struct ChildState {
        reaped: bool,
        journal: bool,
    }
    let mut slot = Some(ChildState {
        reaped: false,
        journal: false,
    });
    let mut uncertain = false;
    let result = fixture::stop_slot(&mut slot, &mut uncertain, async |child| {
        child.reaped = true;
        Err("injected owned journal failure after reap")
    })
    .await;
    assert!(
        result.is_err()
            && slot
                .as_ref()
                .is_some_and(|child| child.reaped && !child.journal),
        "failed stop lost child ownership"
    );
    assert!(uncertain, "stop error did not persist cleanup uncertainty");
    let mut directory = Some(tempfile::tempdir().unwrap());
    let journal = directory.as_ref().unwrap().path().join("ownership.json");
    std::fs::write(&journal, b"owned-fixture-journal").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        fixture::finish_private_directory(&mut directory, true, uncertain, slot.is_some()).is_err()
            && journal.is_file(),
        "failed cleanup removed private journal"
    );
    fixture::stop_slot(&mut slot, &mut uncertain, async |child| {
        child.journal = true;
        if child.reaped && child.journal {
            Ok(())
        } else {
            Err("owned stop still incomplete")
        }
    })
    .await
    .unwrap();
    assert!(
        slot.is_none() && uncertain,
        "successful later stop silently cleared sticky uncertainty"
    );
    assert!(
        fixture::finish_private_directory(&mut directory, true, uncertain, false).is_err()
            && journal.is_file(),
        "other resource cleanup erased earlier uncertainty"
    );
    directory.take().unwrap().close().unwrap();
    let mut verified = Some(tempfile::tempdir().unwrap());
    let path = verified.as_ref().unwrap().path().to_path_buf();
    fixture::finish_private_directory(&mut verified, true, false, false).unwrap();
    assert!(
        verified.is_none() && !path.exists(),
        "fully verified cleanup failed to remove owned directory"
    );
    let task = tokio::spawn(std::future::pending::<TestResult>());
    task.abort();
    let (stop, _stopped) = tokio::sync::oneshot::channel();
    let mut server = fixture::SourceTestServer::task(stop, task);
    assert!(
        server.stop().await.is_err(),
        "aborted owned server task reported verified cleanup"
    );
    let (stop, _stopped) = tokio::sync::oneshot::channel();
    let mut server = fixture::SourceTestServer::task(
        stop,
        tokio::spawn(async { Err("injected listener result failure") }),
    );
    assert!(
        server.stop().await.is_err(),
        "listener error result reported verified cleanup"
    );
    let (stop, _stopped) = tokio::sync::oneshot::channel();
    let mut server = fixture::SourceTestServer::task(stop, tokio::spawn(async { Ok(()) }));
    assert!(
        server.stop().await.is_ok(),
        "successful owned server completion was rejected"
    );
}

#[tokio::test]
#[ignore = "fresh owned two-cluster TLS PostgreSQL and actual Mail/gateway; explicit opt-in required"]
async fn native_and_action_network_replay_conflict_and_restart() -> TestResult {
    for source in Source::ALL {
        Fixture::run(source, catcher::Mode::Accept, async |f| {
            let original = f.input("fixture replay");
            f.discard_public_reply("stable-operation", &original)
                .await?;
            let id = f.only_job().await?;
            f.wait_state(&id, "accepted").await?;
            f.assert_capture(&id, 0).await?;
            let baseline = f.catcher.counts();
            f.restart(false).await?;
            let replay = f.submit("stable-operation", &original).await?;
            assert!(replay["id"] == id, "logical replay changed job");
            let changed = f.input("changed content");
            assert!(
                f.submit_status("stable-operation", &changed).await?
                    == source.changed_input_status(),
                "changed input did not conflict"
            );
            assert!(
                f.only_job().await? == id,
                "changed input altered original canonical job or created another job"
            );
            f.assert_one_message(baseline).await?;
            f.assert_source_scope(&id).await?;
            f.assert_zero_ce().await
        })
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "real Core Endpoint HTTP -> actual Mail Invoke -> managed control -> private PG -> SMTP"]
async fn console_network_large_original_mail_inputs_and_80_character_name() -> TestResult {
    Fixture::run(Source::Action, catcher::Mode::Accept, async |f| {
        for size in [20_000, 30_000] {
            f.faults.hold_claims();
            let mut input = f.input("large original input");
            input["text"] = serde_json::json!("😀".repeat(size));
            input["from_name"] = serde_json::json!("😀".repeat(80));
            let receipt = f.submit(&format!("unicode-{size}"), &input).await?;
            let id = fixture::job_id(&receipt)?;
            f.assert_original_payload(&id, &input).await?;
            f.faults.release();
            f.wait_state(&id, "accepted").await?;
            f.assert_capture(&id, usize::from(size == 30_000)).await?;
            let count = if size == 20_000 { 1 } else { 2 };
            f.assert_no_new_attempts(
                f.catcher.counts(),
                AttemptTotals {
                    data: count,
                    complete: count,
                    stored: count,
                },
            )
            .await?;
        }
        assert!(
            f.catcher.counts().stored == 2,
            "large input was not actually delivered locally"
        );
        f.assert_zero_ce().await
    })
    .await
}

#[tokio::test]
#[ignore = "complete SMTP DATA then lost final reply; actual Core uncertainty and restart"]
async fn native_and_action_unknown_retains_payload_quota_and_never_resends() -> TestResult {
    for source in Source::ALL {
        Fixture::run(source, catcher::Mode::StoreThenDropFinalReply, async |f| {
            let input = f.input("uncertain local acceptance");
            let id = fixture::job_id(&f.submit("uncertain-operation", &input).await?)?;
            f.wait_state(&id, "unknown").await?;
            f.assert_capture(&id, 0).await?;
            let custody = f.retained_custody(&id).await?;
            let baseline = f.catcher.counts();
            f.expire_queue_lease(&id).await?;
            f.restart(true).await?;
            let replay = f.submit("uncertain-operation", &input).await?;
            assert!(
                replay["id"] == id && replay["state"] == "unknown",
                "unknown replay changed intent"
            );
            f.assert_retained_custody(&id, &custody).await?;
            f.assert_one_message(baseline).await?;
            f.assert_transition_denied(&id).await?;
            f.assert_zero_ce().await
        })
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "actual Core settlement for definitive pre/post-DATA SMTP faults"]
async fn native_and_action_explicit_smtp_rejections_are_terminal_and_release_allowance()
-> TestResult {
    for source in Source::ALL {
        for mode in [
            catcher::Mode::RejectBeforeData,
            catcher::Mode::RejectAfterData,
            catcher::Mode::DisconnectBeforeData,
        ] {
            Fixture::run(source, mode, async |f| {
                let input = f.input("definitive local fault");
                let id = fixture::job_id(&f.submit("rejected-operation", &input).await?)?;
                f.wait_state(&id, "rejected").await?;
                f.assert_definitive_rejection(&id, mode).await?;
                let baseline = f.catcher.counts();
                f.restart(false).await?;
                assert!(
                    f.submit("rejected-operation", &input).await?["id"] == id,
                    "rejected replay changed canonical job"
                );
                f.assert_no_new_attempts(baseline, AttemptTotals::rejected(mode)?)
                    .await
            })
            .await?;
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "SIGKILL during actual SMTP handoff after complete DATA; same keys/store restart"]
async fn kill_after_stored_data_restarts_without_a_second_attempt() -> TestResult {
    for source in Source::ALL {
        Fixture::run(source, catcher::Mode::BlockFinalReply, async |f| {
            let input = f.input("killed handoff");
            let id = fixture::job_id(&f.submit("killed-handoff", &input).await?)?;
            f.wait_catcher_blocked().await?;
            f.assert_capture(&id, 0).await?;
            let custody = f.retained_custody(&id).await?;
            let baseline = f.catcher.counts();
            f.restart(true).await?;
            f.expire_queue_lease(&id).await?;
            f.wait_state(&id, "unknown").await?;
            assert!(
                f.submit("killed-handoff", &input).await?["id"] == id,
                "killed handoff replay changed job"
            );
            f.assert_retained_custody(&id, &custody).await?;
            f.assert_one_message(baseline).await
        })
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "transparent ACK suppression only after actual backend commit"]
async fn native_and_action_lost_begin_and_finish_ack_never_duplicate_attempt() -> TestResult {
    for source in Source::ALL {
        for fault in [Fault::LoseBeginReply, Fault::LoseFinishReply] {
            Fixture::run(source, catcher::Mode::Accept, async |f| {
                f.faults.arm(fault);
                let input = f.input("postcommit acknowledgement loss");
                let id = fixture::job_id(&f.submit("lost-ack", &input).await?)?;
                f.wait_fault().await?;
                let baseline = f.catcher.counts();
                let expected = if fault == Fault::LoseBeginReply {
                    AttemptTotals::NONE
                } else {
                    AttemptTotals::ONE_STORED
                };
                if fault == Fault::LoseBeginReply {
                    f.wait_state(&id, "handoff").await?;
                    assert!(
                        f.catcher.counts().stored == 0,
                        "lost begin reply performed SMTP"
                    );
                    f.expire_queue_lease(&id).await?;
                    f.wait_state(&id, "unknown").await?;
                } else {
                    f.wait_state(&id, "accepted").await?;
                    f.assert_capture(&id, 0).await?;
                }
                f.restart(true).await?;
                assert!(
                    f.submit("lost-ack", &input).await?["id"] == id,
                    "ACK replay duplicated job"
                );
                f.assert_no_new_attempts(baseline, expected).await
            })
            .await?;
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "real queued cancellation and source/policy/peer/payload revocation before handoff"]
async fn native_and_action_cancel_and_revocation_fail_closed() -> TestResult {
    for source in Source::ALL {
        for mutation in Revocation::for_source(source) {
            Fixture::run(source, catcher::Mode::Accept, async |f| {
                f.faults.hold_claims();
                let id = fixture::job_id(
                    &f.submit("pending-operation", &f.input("pending intent"))
                        .await?,
                )?;
                f.wait_state(&id, "queued").await?;
                let baseline = f.catcher.counts();
                if mutation == Revocation::Cancel {
                    let response = f.cancel(&id).await?;
                    assert!(
                        response["state"] == "cancelled",
                        "pre-handoff cancellation lost"
                    );
                } else {
                    f.mutate(mutation, &id).await?;
                }
                f.faults.release();
                f.assert_denied_before_handoff(mutation, &id).await?;
                f.assert_no_new_attempts(baseline, AttemptTotals::NONE)
                    .await
            })
            .await?;
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "current peer competing claims, stale token and legacy lane isolation"]
async fn actual_claim_fencing_and_competing_worker_do_not_duplicate_delivery() -> TestResult {
    for source in Source::ALL {
        Fixture::run(source, catcher::Mode::Accept, async |f| {
            f.faults.hold_claims();
            let input = f.input("competing claims");
            let id = fixture::job_id(&f.submit("competing", &input).await?)?;
            f.claim_fencing(&id).await?;
            f.faults.release();
            f.wait_state(&id, "accepted").await?;
            let baseline = f.catcher.counts();
            f.assert_one_message(baseline).await
        })
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "real blocked SMTP drain, disabled bookkeeping and original key/store restart"]
async fn accepted_handoff_blocks_transition_drain_and_survives_sigterm() -> TestResult {
    for source in Source::ALL {
        Fixture::run(source, catcher::Mode::BlockFinalReply, async |f| {
            let id = fixture::job_id(&f.submit("blocked-final", &f.input("bounded drain")).await?)?;
            f.wait_catcher_blocked().await?;
            f.assert_capture(&id, 0).await?;
            let baseline = f.catcher.counts();
            f.assert_transition_denied(&id).await?;
            f.assert_one_use_attempt_replay_denied().await?;
            f.set_installation("draining").await?;
            f.assert_draining_new_admission_denied().await?;
            f.signal_mail_term()?;
            f.catcher.release_final_reply();
            f.wait_mail_stopped().await?;
            f.wait_state(&id, "accepted").await?;
            f.set_installation("disabled").await?;
            f.assert_disabled_and_old_generation(&id).await?;
            f.set_installation("enabled").await?;
            f.restart(false).await?;
            f.assert_one_message(baseline).await
        })
        .await?;
    }
    Ok(())
}

#[tokio::test]
#[ignore = "real Core incoming/domain barrier and shared private cross-process customer fence"]
async fn incoming_and_domain_transition_race_is_serialized_without_ce() -> TestResult {
    Fixture::run(Source::Action, catcher::Mode::Accept, async |f| {
        f.faults.hold_claims();
        let id = fixture::job_id(
            &f.submit("outgoing-first", &f.input("outstanding direct intent"))
                .await?,
        )?;
        f.incoming_race(&id).await?;
        f.faults.release();
        f.wait_state(&id, "accepted").await?;
        f.assert_one_message(f.catcher.counts()).await?;
        f.assert_zero_ce().await
    })
    .await
}

#[tokio::test]
#[ignore = "actual automatic binding and real canonical custody reader fence"]
async fn normal_tls_binding_derivation_and_schema_reader_fence() -> TestResult {
    Fixture::run(Source::Action, catcher::Mode::Accept, async |f| {
        f.generic_material_and_reader_fence().await
    })
    .await
}

#[tokio::test]
#[ignore = "public signed install and actual generic Worker advance; no Kubernetes"]
async fn generic_installer_persisted_six_files_run_actual_mail_with_automatic_binding() -> TestResult
{
    Fixture::run_installer().await
}
