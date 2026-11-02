use super::*;
impl DatabaseService {
    /// Called only after the HTTP boundary authenticates a real Principal and server-owned proof.
    pub async fn execute(
        &self,
        principal: &Principal,
        authority: &StoredAuthority,
        input: Value,
    ) -> Result<Value> {
        let cmd = Command::parse(input)?;
        let (auth, current) = self
            .verify(&principal.user_id, &principal.session_id, authority)
            .await?;
        let tx = self.begin_actor(&current).await?;
        self.installed(&tx, cmd.mutates()).await?;
        let value = match cmd {
            Command::List { customer_id } => {
                let access = self
                    .authorize(
                        &tx,
                        &current,
                        None,
                        Some(&customer_id),
                        false,
                        false,
                        authority,
                    )
                    .await?;
                let operator = access["operator"] == true;
                let bindings=all(&tx,&format!("{SELECTION} AND p.customer_id=$2 AND ($3::boolean OR g.id IS NOT NULL) ORDER BY b.name,b.id LIMIT 200"),vec![current.user_id.clone().into(),customer_id.clone().into(),operator.into()]).await?;
                let projects = if operator {
                    all(&tx,"SELECT jsonb_build_object('id',id,'name',name) AS value FROM webdock_admin.projects WHERE customer_id=$1 AND status='active' ORDER BY name,id LIMIT 200",vec![customer_id.clone().into()]).await?
                } else {
                    vec![]
                };
                let people = if operator {
                    all(&auth,"SELECT jsonb_build_object('id',u.id,'name',u.name,'email',u.email) AS value FROM webdock_auth.\"user\" u WHERE NOT coalesce(u.banned,false) AND u.\"emailVerified\" AND webdock_rls.local_identity_authority(u.id) AND (u.role='operator' OR EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" WHERE m.\"userId\"=u.id AND t.customer_id=$1 AND webdock_rls.local_organization_authority(t.organization_id))) ORDER BY u.name,u.id LIMIT 200",vec![customer_id.clone().into()]).await?
                } else {
                    vec![]
                };
                let managed=all(&tx,"SELECT jsonb_build_object('id',d.id,'projectID',d.project_id,'name',d.name,'state',d.state,'canOpen',($2::boolean AND d.state='ready' AND d.pending_backend_id IS NULL AND d.confirmed_credentials_encrypted IS NOT NULL AND d.confirmed_credential_revision=d.credential_revision AND b.state='ready' AND b.endpoint_ready AND coalesce(b.ca_certificate,'')<>'')) AS value FROM webdock_auth.hosting_database d JOIN webdock_admin.projects p ON p.id=d.project_id JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.customer_id=$1 AND p.status='active' AND d.state<>'purged' ORDER BY d.name,d.id LIMIT 200",vec![customer_id.into(),(access["canWrite"]==true&&!authority.read_only()).into()]).await?;
                json!({"bindings":bindings,"managed":managed,"operator":operator,"projects":projects,"people":people})
            }
            Command::Create {
                project_id,
                name,
                environment,
                engine,
            } => {
                self.authorize(
                    &tx,
                    &current,
                    Some(&project_id),
                    None,
                    true,
                    true,
                    authority,
                )
                .await?;
                let id = self.id().await?;
                exec(&tx,"INSERT INTO webdock_auth.database_binding(id,project_id,name,environment,engine) VALUES($1,$2,$3,$4,$5)",vec![id.clone().into(),project_id.into(),name.into(),environment.into(),engine.into()]).await?;
                self.audit(&tx, &current, "binding.create", &id).await?;
                json!({"id":id})
            }
            Command::OpenManaged { database_id } => {
                self.open_managed(&tx, &current, authority, &database_id)
                    .await?
            }
            other => {
                self.binding_command(&tx, &auth, &current, authority, other)
                    .await?
            }
        };
        tx.commit().await?;
        auth.commit().await?;
        Ok(value)
    }
    async fn binding_command(
        &self,
        tx: &DatabaseTransaction,
        auth: &DatabaseTransaction,
        current: &Principal,
        authority: &StoredAuthority,
        command: Command,
    ) -> Result<Value> {
        let (Command::Get { binding_id }
        | Command::Grant { binding_id, .. }
        | Command::Revoke { binding_id, .. }
        | Command::Disable { binding_id }
        | Command::Open { binding_id }) = &command
        else {
            return Err(invalid());
        };
        let management = matches!(
            command,
            Command::Grant { .. } | Command::Revoke { .. } | Command::Disable { .. }
        );
        let row = one(
            tx,
            &format!("{SELECTION} AND b.id=$2 FOR SHARE OF b"),
            vec![current.user_id.clone().into(), binding_id.clone().into()],
        )
        .await?;
        self.authorize(
            tx,
            current,
            Some(text(&row, "projectID")?),
            None,
            management,
            management,
            authority,
        )
        .await?;
        if !management && row["profile"].is_null() {
            return Err(denied());
        }
        match command {
            Command::Get { .. } => Ok(row),
            Command::Grant {
                binding_id,
                subject,
                profile,
                runtime_origin,
                connection_id,
                proxy_secret,
                ..
            } => {
                one(auth,"SELECT jsonb_build_object('id',u.id) AS value FROM webdock_auth.\"user\" u WHERE u.id=$1 AND NOT coalesce(u.banned,false) AND u.\"emailVerified\" AND webdock_rls.local_identity_authority(u.id) AND (u.role='operator' OR EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m.\"organizationId\" WHERE m.\"userId\"=u.id AND t.customer_id=$2 AND webdock_rls.local_organization_authority(t.organization_id))) FOR SHARE OF u",vec![subject.clone().into(),text(&row,"customerID")?.into()]).await?;
                let old=optional(tx,"SELECT to_jsonb(g) AS value FROM webdock_auth.database_grant g WHERE binding_id=$1 AND subject=$2 FOR UPDATE",vec![binding_id.clone().into(),subject.clone().into()]).await?;
                if old.is_some_and(|o| {
                    o["profile"] != profile && o["runtime_origin"] == runtime_origin
                }) {
                    return Err(Error {
                        status: 409,
                        message: "Changing database privileges requires a newly isolated runtime.",
                    });
                }
                let claimed=optional(tx,"INSERT INTO webdock_auth.database_runtime_owner(runtime_origin,binding_id,subject,profile) VALUES($1,$2,$3,$4) ON CONFLICT(runtime_origin) DO UPDATE SET runtime_origin=EXCLUDED.runtime_origin WHERE database_runtime_owner.binding_id=EXCLUDED.binding_id AND database_runtime_owner.subject=EXCLUDED.subject AND database_runtime_owner.profile=EXCLUDED.profile RETURNING jsonb_build_object('origin',runtime_origin) AS value",vec![runtime_origin.clone().into(),binding_id.clone().into(),subject.clone().into(),profile.clone().into()]).await?;
                if claimed.is_none() {
                    return Err(Error {
                        status: 409,
                        message: "This runtime is already assigned to another database user.",
                    });
                }
                let sealed = self
                    .cipher
                    .seal(&proxy_secret, &format!("database:{binding_id}:{subject}"))
                    .map_err(|_| unavailable())?;
                exec(tx,"INSERT INTO webdock_auth.database_grant(id,binding_id,subject,profile,runtime_origin,connection_id,secret_sealed) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT(binding_id,subject) DO UPDATE SET profile=EXCLUDED.profile,runtime_origin=EXCLUDED.runtime_origin,connection_id=EXCLUDED.connection_id,secret_sealed=EXCLUDED.secret_sealed,revision=database_grant.revision+1,enabled=true",vec![self.id().await?.into(),binding_id.clone().into(),subject.into(),profile.into(),runtime_origin.into(),connection_id.into(),sealed.into()]).await?;
                self.audit(tx, current, "grant.configure", &binding_id)
                    .await?;
                Ok(json!({"ok":true}))
            }
            Command::Revoke {
                binding_id,
                subject,
            } => {
                exec(tx,"UPDATE webdock_auth.database_grant SET enabled=false,revision=revision+1 WHERE binding_id=$1 AND subject=$2",vec![binding_id.clone().into(),subject.into()]).await?;
                self.audit(tx, current, "revoke", &binding_id).await?;
                Ok(json!({"ok":true}))
            }
            Command::Disable { binding_id } => {
                exec(
                    tx,
                    "UPDATE webdock_auth.database_binding SET enabled=false WHERE id=$1",
                    vec![binding_id.clone().into()],
                )
                .await?;
                exec(tx,"UPDATE webdock_auth.database_grant SET enabled=false,revision=revision+1 WHERE binding_id=$1",vec![binding_id.clone().into()]).await?;
                self.audit(tx, current, "disable", &binding_id).await?;
                Ok(json!({"ok":true}))
            }
            Command::Open { binding_id } => {
                if authority.read_only() && row["profile"] != "read" {
                    return Err(denied());
                }
                let code = token()?;
                let reference = serde_json::to_value(authority).map_err(|_| unavailable())?;
                let launch=one(tx,"INSERT INTO webdock_auth.database_launch(token_hash,grant_id,grant_revision,session_id,expires_at,native_authority) SELECT $1,id,revision,$2,least(now()+interval '60 seconds',$6),$5 FROM webdock_auth.database_grant WHERE binding_id=$3 AND subject=$4 AND enabled RETURNING jsonb_build_object('expiresAt',expires_at) AS value",vec![hash(&code)?.into(),current.session_id.clone().into(),binding_id.clone().into(),current.user_id.clone().into(),reference.into(),current.expires_at.into()]).await?;
                self.audit(tx, current, "session.launch", &binding_id)
                    .await?;
                Ok(
                    json!({"code":code,"gatewayOrigin":self.gateway_origin,"binding":row,"expiresAt":launch["expiresAt"]}),
                )
            }
            _ => Err(invalid()),
        }
    }
    pub(super) async fn managed(
        &self,
        tx: &DatabaseTransaction,
        current: &Principal,
        authority: &StoredAuthority,
        id: &str,
    ) -> Result<Value> {
        let d=one(tx,"SELECT to_jsonb(d)||jsonb_build_object('host',b.host,'direct_port',b.direct_port,'endpoint_ready',b.endpoint_ready,'ca_certificate',b.ca_certificate,'backend_state',b.state) AS value FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.id=$1 FOR SHARE OF d,b",vec![id.into()]).await?;
        self.authorize(
            tx,
            current,
            Some(text(&d, "project_id")?),
            None,
            true,
            false,
            authority,
        )
        .await?;
        if d["state"] != "ready"
            || !d["pending_backend_id"].is_null()
            || d["backend_state"] != "ready"
            || d["endpoint_ready"] != true
            || text(&d, "ca_certificate")?.is_empty()
            || d["confirmed_credentials_encrypted"].is_null()
            || d["confirmed_credential_revision"] != d["credential_revision"]
        {
            return Err(denied());
        }
        Ok(d)
    }
    async fn open_managed(
        &self,
        tx: &DatabaseTransaction,
        current: &Principal,
        authority: &StoredAuthority,
        id: &str,
    ) -> Result<Value> {
        let d = self.managed(tx, current, authority, id).await?;
        let code = token()?;
        let launch=one(tx,"INSERT INTO webdock_auth.managed_database_launch(token_hash,database_id,backend_id,database_revision,credential_revision,subject,session_id,expires_at,native_authority) VALUES($1,$2,$3,$4,$5,$6,$7,least(now()+interval '60 seconds',$9),$8) RETURNING jsonb_build_object('expiresAt',expires_at) AS value",vec![hash(&code)?.into(),id.into(),text(&d,"backend_id")?.into(),number(&d,"revision")?.into(),number(&d,"confirmed_credential_revision")?.into(),current.user_id.clone().into(),current.session_id.clone().into(),serde_json::to_value(authority).map_err(|_|unavailable())?.into(),current.expires_at.into()]).await?;
        exec(tx,"INSERT INTO webdock_auth.hosting_audit(id,subject,action,target_id,outcome) VALUES($1,$2,'databases.viewer.launch',$3,'success')",vec![self.id().await?.into(),current.user_id.clone().into(),id.into()]).await?;
        Ok(
            json!({"code":code,"gatewayOrigin":self.gateway_origin,"binding":{"id":id,"projectID":d["project_id"],"customerID":d["customer_id"],"name":d["name"],"environment":"production","engine":"postgresql","profile":"schema"},"expiresAt":launch["expiresAt"]}),
        )
    }
}
