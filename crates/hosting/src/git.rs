//! Trusted build-service SPI. Caller owns the worker transaction; no browser principal is synthesized.
use super::*;
const GIT_ENVIRONMENT_SOURCE: &str = "SELECT to_jsonb(s) AS value FROM webdock_auth.git_source s WHERE id=$1 AND recipe IN ('dockerfile','dockerfile-frontend') FOR SHARE";
const GIT_PUBLICATION_SOURCE: &str = "SELECT to_jsonb(h) AS value FROM webdock_auth.git_source s JOIN webdock_auth.hosting_project h ON h.project_id=s.project_id WHERE s.id=$1 AND s.recipe IN ('dockerfile','dockerfile-frontend')";
impl HostingService {
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )]
    pub(super) async fn authorize_git(
        tx: &DatabaseTransaction,
        actor: &HostingActor,
        release: &str,
        project: Option<&str>,
        customer: Option<&str>,
        write: bool,
        operator: bool,
        live: bool,
    ) -> Result<Access> {
        if !write || operator || live {
            return Err(error(403, "Git publication only admits deployment writes."));
        }
        let authority = rows(
            tx,
            "SELECT coalesce(webdock_rls.git_release_authority($1),'null'::jsonb) AS value",
            vec![release.into()],
        )
        .await?
        .pop()
        .unwrap_or(Value::Null);
        if authority.is_null()
            || authority["subject"] != actor.user_id
            || project.is_none_or(|p| authority["projectID"] != p)
            || customer.is_some_and(|c| authority["customerID"] != c)
        {
            return Err(error(403, "Git publication authority is no longer valid."));
        }
        let project = project.unwrap();
        one(
            tx,
            "SELECT webdock_admin.lock_hosting_project($1)",
            vec![project.into()],
        )
        .await?;
        let target=rows(tx,"SELECT to_jsonb(p)||to_jsonb(h) AS value FROM webdock_admin.projects p JOIN webdock_auth.hosting_project h ON h.project_id=p.id WHERE p.id=$1 AND p.status='active'",vec![project.into()]).await?.pop().ok_or_else(||error(404,"Git hosting target is unavailable."))?;
        Ok(Access {
            customer: field(&authority, "customerID")?.into(),
            project: target,
            operator: authority["operator"] == true,
            live: true,
            can_write: true,
        })
    }
    /// Computes the retained v1 source snapshot in the caller's signed RLS transaction.
    /// This is backend-only; it never returns environment values to an HTTP client.
    pub async fn git_environment_snapshot(
        &self,
        tx: &DatabaseTransaction,
        source_id: &str,
    ) -> Result<(String, i64)> {
        let source = rows(tx, GIT_ENVIRONMENT_SOURCE, vec![source_id.into()])
            .await?
            .pop()
            .ok_or_else(|| error(404, "Git source is unavailable."))?;
        let app = Self::app(tx, field(&source, "target_id")?).await?;
        if app["project_id"] != source["project_id"] || app["status"] == "deleted" {
            return Err(error(404, "Git target is unavailable."));
        }
        let environment = self
            .environment
            .open(field(&app, "id")?, app["environment_encrypted"].as_str())?;
        let build_environment = self.environment.open(
            &format!("git-build:{source_id}"),
            source["build_environment_encrypted"].as_str(),
        )?;
        Ok((
            snapshot_hash(&environment, &app["spec"], &build_environment)?,
            numeric(&app, "revision")?,
        ))
    }
    pub async fn git_build_environment(
        &self,
        tx: &DatabaseTransaction,
        source_id: &str,
    ) -> Result<std::collections::BTreeMap<String, String>> {
        let row = rows(
            tx,
            "SELECT to_jsonb(s) AS value FROM webdock_auth.git_source s WHERE id=$1",
            vec![source_id.into()],
        )
        .await?
        .pop()
        .ok_or_else(|| error(404, "Git source unavailable."))?;
        self.environment.open(
            &format!("git-build:{source_id}"),
            row["build_environment_encrypted"].as_str(),
        )
    }
    /// Atomically stage a previously published OCI artifact into the hosting app and operation.
    /// `tx` must carry the private git-worker scope. Worker, source, release, artifact and approval
    /// are checked here independently; callers must commit this same transaction after success.
    pub async fn publish_git_release(
        &self,
        tx: &DatabaseTransaction,
        release_id: &str,
        worker_id: &str,
        worker_generation: i32,
        release_generation: i32,
        image: &str,
    ) -> Result<Value> {
        self.publish_git_release_with_authority_in(
            tx,
            None,
            release_id,
            worker_id,
            worker_generation,
            release_generation,
            image,
        )
        .await
    }
    /// Reuses the caller's held authentication transaction and independently verifies the
    /// persisted approval. This prevents nested auth-pool acquisition at publication.
    #[expect(
        clippy::too_many_arguments,
        reason = "Explicit transaction, authority and revision inputs keep this operation boundary auditable."
    )]
    pub async fn publish_git_release_with_authority_in(
        &self,
        tx: &DatabaseTransaction,
        authority_tx: Option<&DatabaseTransaction>,
        release_id: &str,
        worker_id: &str,
        worker_generation: i32,
        release_generation: i32,
        image: &str,
    ) -> Result<Value> {
        contracts::record_id(release_id)?;
        contracts::record_id(worker_id)?;
        let row=rows(tx,"SELECT to_jsonb(r)||jsonb_build_object('image_digest',a.image_digest,'source_target',s.target_id) AS value FROM webdock_auth.git_release r JOIN webdock_auth.git_source s ON s.id=r.source_id JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id JOIN webdock_auth.git_worker w ON w.id=r.worker_id WHERE r.id=$1 AND webdock_rls.worker(ARRAY['git-worker']) AND r.status='deploying' AND r.worker_id=$2 AND r.worker_generation=$3 AND r.generation=$4 AND r.lease_until>now() AND w.enabled AND w.generation=$3 AND a.retained AND a.kind='oci' AND r.publication_started_at IS NOT NULL FOR UPDATE OF r,s",vec![release_id.into(),worker_id.into(),worker_generation.into(),release_generation.into()]).await?.pop().ok_or_else(||error(409,"Git publication lease is unavailable."))?;
        let repository = row["publication_target"]["imageRepository"]
            .as_str()
            .ok_or_else(|| error(409, "Git registry publication has not been prepared."))?;
        let digest = field(&row, "image_digest")?;
        if image != format!("{repository}@{digest}") {
            return Err(error(
                409,
                "Published image does not match the verified artifact.",
            ));
        }
        let authority = rows(
            tx,
            "SELECT coalesce(webdock_rls.git_release_authority($1),'null'::jsonb) AS value",
            vec![release_id.into()],
        )
        .await?
        .pop()
        .unwrap_or(Value::Null);
        if authority.is_null() {
            return Err(error(403, "Git approval is no longer valid."));
        }
        let browser_guard = if authority_tx.is_none() {
            self.release_browser_guard(&row).await?
        } else {
            None
        };
        self.verify_release_browser_in(&row, authority_tx).await?;
        if self
            .git_environment_snapshot(tx, field(&row, "source_id")?)
            .await?
            .0
            != row["environment_identity"].as_str().unwrap_or("")
        {
            return Err(error(409, "Git application environment changed."));
        }
        if let Some(id) = row["operation_id"].as_str() {
            let op=rows(tx,"SELECT to_jsonb(o) AS value FROM webdock_auth.hosting_operation o WHERE id=$1 AND git_release_id=$2",vec![id.into(),release_id.into()]).await?.pop().ok_or_else(||error(409,"Git operation reference is invalid."))?;
            return Ok(
                json!({"id":op["id"],"appID":op["app_id"],"status":op["status"],"revision":op["target_revision"]}),
            );
        }
        self.apply_scope(
            tx,
            "webdock_worker_runtime",
            json!({"kind":"system","purpose":"plugin-admission","authProvider":"better-auth"}),
        )
        .await?;
        Self::module(tx, true, false).await?;
        self.apply_scope(
            tx,
            "webdock_worker_runtime",
            json!({"kind":"system","purpose":"git-worker","authProvider":"better-auth"}),
        )
        .await?;
        let actor = HostingActor {
            user_id: field(&authority, "subject")?.into(),
            session_id: authority["sessionID"].as_str().unwrap_or("").into(),
            git_release: Some(release_id.into()),
            customer_scope: None,
        };
        let app = Self::app(tx, field(&row, "source_target")?).await?;
        let mut spec = app["spec"].clone();
        spec["image"] = json!(image);
        spec["template"] = json!("custom");
        let subscription=Self::limits(tx,field(&row,"customer_id")?,None,None).await?["subscriptionRevision"].clone();
        let cmd = contracts::parse_command(
            json!({"action":if app["lifecycle"]=="draft"{"apps.deploy"}else{"apps.update"},"appID":app["id"],"revision":app["revision"],"subscriptionRevision":subscription,"spec":spec,"idempotencyKey":format!("git-release-{release_id}")}),
        )?;
        let (result, _) = self.apps(tx, &actor, &cmd).await?;
        execute(
            tx,
            "UPDATE webdock_auth.hosting_operation SET git_release_id=$2,desired=desired||jsonb_build_object('gitApprovalAuthority',$3::jsonb,'gitApprovalSubject',$4::text,'gitApprovalSession',$5::text) WHERE id=$1",
            vec![field(&result, "id")?.into(), release_id.into(),row["publication_target"]["approvedAuthority"].clone().into(),row["approved_by"].as_str().into(),row["approved_session_id"].as_str().into()],
        )
        .await?;
        execute(
            tx,
            "UPDATE webdock_auth.git_release SET operation_id=$2 WHERE id=$1",
            vec![release_id.into(), field(&result, "id")?.into()],
        )
        .await?;
        Self::authorize_git(
            tx,
            &actor,
            release_id,
            Some(field(&row, "project_id")?),
            None,
            true,
            false,
            false,
        )
        .await?;
        browser_authority::verify(&browser_guard).await?;
        self.verify_release_browser_in(&row, authority_tx).await?;
        Ok(result)
    }
}
impl HostingService {
    /// Backend-only environment patch, using the retained source-bound AEAD envelope.
    pub async fn patch_git_build_environment(
        &self,
        tx: &DatabaseTransaction,
        source_id: &str,
        patch: &Value,
    ) -> Result<()> {
        let previous = self.git_build_environment(tx, source_id).await?;
        let updated = Environment::patch(previous, patch)?;
        let encrypted = self
            .environment
            .seal(&format!("git-build:{source_id}"), &updated)?;
        execute(
            tx,
            "UPDATE webdock_auth.git_source SET build_environment_encrypted=$2 WHERE id=$1",
            vec![source_id.into(), encrypted.into()],
        )
        .await?;
        Ok(())
    }
}
impl HostingService {
    pub async fn git_publication_ready(
        &self,
        tx: &DatabaseTransaction,
        source_id: &str,
    ) -> Result<()> {
        let row = rows(tx, GIT_PUBLICATION_SOURCE, vec![source_id.into()])
            .await?
            .pop()
            .ok_or_else(|| error(404, "Git hosting target is unavailable."))?;
        let snapshot = Self::cluster_view(tx, field(&row, "cluster_id")?).await?;
        if snapshot["workloadReady"] != true
            || snapshot["observation"]["capabilities"]["registryVersion"] != 1
        {
            return Err(error(
                503,
                "Verified cluster registry support is unavailable.",
            ));
        }
        Ok(())
    }
}

// Retained v1 compatibility boundary: JSON.stringify's literal field order, ASCII runtime keys,
// and integral JSON number rendering. Images/templates are deliberately excluded from identity.
fn snapshot_hash(
    environment: &std::collections::BTreeMap<String, String>,
    spec: &Value,
    build_environment: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
    fn normalize(value: &mut Value) -> Result<()> {
        match value {
            Value::Number(n) => {
                let n = n
                    .as_f64()
                    .filter(|n| {
                        n.is_finite() && n.fract() == 0.0 && n.abs() <= contracts::MAX_SAFE as f64
                    })
                    .ok_or_else(|| error(503, "Invalid stored runtime quantity."))?;
                *value = if n >= 0.0 {
                    json!(n as u64)
                } else {
                    json!(n as i64)
                };
            }
            Value::Array(values) => {
                for v in values {
                    normalize(v)?;
                }
            }
            Value::Object(values) => {
                for v in values.values_mut() {
                    normalize(v)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut spec = spec.clone();
    normalize(&mut spec)?;
    let runtime = spec
        .as_object()
        .ok_or_else(|| error(503, "Stored app specification invalid."))?
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "image" | "template"))
        .collect();
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Snapshot<'a> {
        environment: Vec<(&'a String, &'a String)>,
        runtime: std::collections::BTreeMap<&'a String, &'a Value>,
        build_environment: Vec<(&'a String, &'a String)>,
    }
    let snapshot = Snapshot {
        environment: environment.iter().collect(),
        runtime,
        build_environment: build_environment.iter().collect(),
    };
    Ok(hex::encode(Sha256::digest(
        serde_json::to_vec(&snapshot).map_err(|_| error(503, "Snapshot unavailable."))?,
    )))
}
#[cfg(test)]
mod compatibility {
    use super::*;
    #[tokio::test]
    #[ignore = "minimal owned PostgreSQL query fixture; no native bootstrap"]
    async fn frontend_source_environment_query() {
        assert_recipe_query(GIT_ENVIRONMENT_SOURCE, "environment snapshot").await;
    }
    #[tokio::test]
    #[ignore = "minimal owned PostgreSQL query fixture; no native bootstrap"]
    async fn frontend_source_publication_query() {
        assert_recipe_query(GIT_PUBLICATION_SOURCE, "publication readiness").await;
    }
    async fn assert_recipe_query(query: &str, boundary: &str) {
        let url =
            std::env::var("WEBDOCK_GIT_RECIPE_DATABASE_URL").expect("explicit owned query fixture");
        let db = sea_orm::Database::connect(url).await.unwrap();
        let tx = db.begin().await.unwrap();
        for (id, recipe) in [
            ("100", "dockerfile"),
            ("101", "dockerfile-frontend"),
            ("102", "vercel"),
            ("103", "unknown"),
        ] {
            execute(
                &tx,
                "INSERT INTO webdock_auth.git_source(id,project_id,recipe) VALUES($1,'200',$2)",
                vec![id.into(), recipe.into()],
            )
            .await
            .unwrap();
        }
        execute(
            &tx,
            "INSERT INTO webdock_auth.hosting_project(project_id,cluster_id) VALUES('200','300')",
            vec![],
        )
        .await
        .unwrap();
        for (id, expected) in [
            ("100", true),
            ("101", true),
            ("102", false),
            ("103", false),
            ("999", false),
        ] {
            let actual = rows(&tx, query, vec![id.into()]).await.unwrap();
            assert_eq!(!actual.is_empty(), expected, "{boundary} source {id}");
        }
        tx.rollback().await.unwrap();
    }
    #[test]
    fn retained_node_snapshot_vectors() {
        let environment = std::collections::BTreeMap::new();
        let build = std::collections::BTreeMap::from([(
            "BUILD_TOKEN".into(),
            "sensitive-fixture-123".into(),
        )]);
        let mut spec = json!({"template":"custom","image":"ignored","args":[],"cpuMillicores":100.0,"ephemeralBytes":1048576,"healthPath":"/","memoryBytes":16777216,"port":8080,"replicas":1,"volumeBytes":0});
        assert_eq!(
            snapshot_hash(&environment, &spec, &build).unwrap(),
            "9b32161381e1df7e744767ca6bceb87cd5b93e1fd57672b630200145e7a9764d"
        );
        spec["args"] = json!(["A\"\\\n☃"]);
        spec["volumeBytes"] = json!(-0.0);
        assert_eq!(
            snapshot_hash(&environment, &spec, &build).unwrap(),
            "67f681b83333ab8dcfd70c5b4efe58e616f09c2f1a745a4752cce6746ec58bfb"
        );
    }
}
