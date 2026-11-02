use super::*;
impl DatabaseService {
    /// Confidential gateway endpoint; caller authenticates the configured gateway shared secret.
    pub async fn exchange(&self, code: &str) -> Result<Value> {
        let hashed = hash(code)?;
        let worker = self.begin_worker().await?;
        let regular=optional(&worker,"UPDATE webdock_auth.database_launch SET consumed_at=now() WHERE token_hash=$1 AND consumed_at IS NULL AND expires_at>now() RETURNING to_jsonb(database_launch) AS value",vec![hashed.clone().into()]).await?;
        let managed = regular.is_none();
        let launch=match regular{Some(v)=>v,None=>one(&worker,"UPDATE webdock_auth.managed_database_launch SET consumed_at=now() WHERE token_hash=$1 AND consumed_at IS NULL AND expires_at>now() RETURNING to_jsonb(managed_database_launch) AS value",vec![hashed.into()]).await?};
        let (auth, business, principal, _details) =
            self.authorize_saved(&worker, &launch, managed).await?;
        let session_token = token()?;
        let id = self.id().await?;
        let value = if managed {
            one(&worker,"INSERT INTO webdock_auth.managed_database_session(token_hash,id,database_id,backend_id,database_revision,credential_revision,subject,session_id,expires_at,native_authority) VALUES($1,$2,$3,$4,$5,$6,$7,$8,least(now()+interval '15 minutes',$10),$9) RETURNING jsonb_build_object('scopeID',id,'expiresAt',expires_at) AS value",vec![hash(&session_token)?.into(),id.into(),text(&launch,"database_id")?.into(),text(&launch,"backend_id")?.into(),number(&launch,"database_revision")?.into(),number(&launch,"credential_revision")?.into(),principal.user_id.into(),principal.session_id.into(),launch["native_authority"].clone().into(),principal.expires_at.into()]).await?
        } else {
            one(&worker,"INSERT INTO webdock_auth.database_session(token_hash,id,grant_id,grant_revision,session_id,expires_at,native_authority) VALUES($1,$2,$3,$4,$5,least(now()+interval '15 minutes',$7),$6) RETURNING jsonb_build_object('scopeID',id,'expiresAt',expires_at) AS value",vec![hash(&session_token)?.into(),id.into(),text(&launch,"grant_id")?.into(),number(&launch,"grant_revision")?.into(),principal.session_id.into(),launch["native_authority"].clone().into(),principal.expires_at.into()]).await?
        };
        worker.commit().await?;
        business.commit().await?;
        auth.commit().await?;
        Ok(json!({"token":session_token,"scopeID":value["scopeID"],"expiresAt":value["expiresAt"]}))
    }
    pub async fn inspect(&self, token: &str) -> Result<Value> {
        let hashed = hash(token)?;
        let worker = self.begin_worker().await?;
        let regular=optional(&worker,"SELECT to_jsonb(s) AS value FROM webdock_auth.database_session s WHERE token_hash=$1 AND expires_at>now()",vec![hashed.clone().into()]).await?;
        let managed = regular.is_none();
        let session=match regular{Some(v)=>v,None=>one(&worker,"SELECT to_jsonb(s) AS value FROM webdock_auth.managed_database_session s WHERE token_hash=$1 AND expires_at>now()",vec![hashed.into()]).await?};
        let (auth, business, principal, details) =
            self.authorize_saved(&worker, &session, managed).await?;
        let result = if managed {
            let secret: Value = self
                .cipher
                .open(
                    text(&details, "confirmed_credentials_encrypted")?,
                    &format!(
                        "database:{}:{}",
                        text(&details, "id")?,
                        number(&details, "confirmed_credential_revision")?
                    ),
                )
                .map_err(|_| denied())?;
            let password = text(&secret, "migrationPassword")?;
            if password.is_empty() || password.len() > 4096 {
                return Err(denied());
            }
            json!({"kind":"managed","scopeID":session["id"],"subject":principal.user_id,"connectionID":details["id"],"profile":"schema","expiresAt":session["expires_at"],"revision":format!("{}:{}:{}",text(&details,"backend_id")?,number(&details,"confirmed_credential_revision")?,number(&details,"revision")?),"database":{"host":details["host"],"port":details["direct_port"],"database":details["database_name"],"username":format!("wd_migrate_{}",text(&details,"id")?),"password":password,"caCertificate":details["ca_certificate"]}})
        } else {
            let secret: String = self
                .cipher
                .open(
                    text(&details, "secret_sealed")?,
                    &format!(
                        "database:{}:{}",
                        text(&details, "binding_id")?,
                        principal.user_id
                    ),
                )
                .map_err(|_| denied())?;
            if !(32..=512).contains(&secret.len()) {
                return Err(denied());
            }
            json!({"scopeID":session["id"],"subject":principal.user_id,"connectionID":details["connection_id"],"profile":details["profile"],"runtimeOrigin":normalize_origin(text(&details,"runtime_origin")?)?,"proxySecret":secret,"expiresAt":session["expires_at"]})
        };
        worker.commit().await?;
        business.commit().await?;
        auth.commit().await?;
        Ok(result)
    }
    pub async fn logout(&self, token: &str) -> Result<Value> {
        let hashed = hash(token)?;
        let tx = self.begin_worker().await?;
        exec(
            &tx,
            "DELETE FROM webdock_auth.database_session WHERE token_hash=$1",
            vec![hashed.clone().into()],
        )
        .await?;
        exec(
            &tx,
            "DELETE FROM webdock_auth.managed_database_session WHERE token_hash=$1",
            vec![hashed.into()],
        )
        .await?;
        tx.commit().await?;
        Ok(json!({"ok":true}))
    }
    async fn authorize_saved(
        &self,
        worker: &DatabaseTransaction,
        saved: &Value,
        managed: bool,
    ) -> Result<(DatabaseTransaction, DatabaseTransaction, Principal, Value)> {
        // External-provider rows are never interpreted as canonical local session proof.
        if !saved["clerk_identity"].is_null() {
            return Err(denied());
        }
        let authority = if saved["native_authority"].is_null() {
            StoredAuthority::LocalSession {}
        } else {
            serde_json::from_value::<StoredAuthority>(saved["native_authority"].clone())
                .map_err(|_| denied())?
        };
        let grant = if managed {
            Value::Null
        } else {
            one(worker,"SELECT to_jsonb(g)||jsonb_build_object('project_id',b.project_id) AS value FROM webdock_auth.database_grant g JOIN webdock_auth.database_binding b ON b.id=g.binding_id JOIN webdock_auth.database_runtime_owner o ON o.runtime_origin=g.runtime_origin AND o.binding_id=g.binding_id AND o.subject=g.subject AND o.profile=g.profile WHERE g.id=$1 AND g.revision=$2 AND g.enabled AND b.enabled FOR SHARE OF g,b,o",vec![text(saved,"grant_id")?.into(),number(saved,"grant_revision")?.into()]).await?
        };
        let subject = if managed {
            text(saved, "subject")?
        } else {
            text(&grant, "subject")?
        };
        let (auth, current) = self
            .verify(subject, text(saved, "session_id")?, &authority)
            .await?;
        let tx = self.begin_actor(&current).await?;
        self.installed(&tx, false).await?;
        let details = if managed {
            let d = self
                .managed(&tx, &current, &authority, text(saved, "database_id")?)
                .await?;
            if d["backend_id"] != saved["backend_id"]
                || d["revision"] != saved["database_revision"]
                || d["confirmed_credential_revision"] != saved["credential_revision"]
            {
                return Err(denied());
            }
            d
        } else {
            if authority.read_only() && grant["profile"] != "read" {
                return Err(denied());
            }
            self.authorize(
                &tx,
                &current,
                Some(text(&grant, "project_id")?),
                None,
                false,
                false,
                &authority,
            )
            .await?;
            grant
        };
        Ok((auth, tx, current, details))
    }
}
