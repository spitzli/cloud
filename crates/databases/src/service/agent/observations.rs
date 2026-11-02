use super::*;
impl DatabaseService {
    pub async fn database_observation_config(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
    ) -> Result<Value> {
        self.agent_scope(&tx, agent).await?;
        let capability=one(&tx,"SELECT jsonb_build_object('enabled',observation#>'{capabilities,databaseVersion}'='1'::jsonb) AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1",vec![agent.cluster_id().into()]).await?;
        if capability["enabled"] != true {
            tx.commit().await?;
            return Ok(json!({"installations":[]}));
        }
        let backends=all(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE cluster_id=$1 AND state<>'purged' AND cnpg_uid IS NOT NULL ORDER BY id LIMIT 100",vec![agent.cluster_id().into()]).await?;
        let mut result = vec![];
        for b in backends {
            let members=all(&tx,"SELECT to_jsonb(id) AS value FROM webdock_auth.hosting_database WHERE backend_id=$1 AND state<>'purged' ORDER BY id LIMIT 100",vec![text(&b,"id")?.into()]).await?;
            result.push(json!({"id":b["id"],"namespace":format!("wd-db-{}",text(&b,"id")?),"cnpgUID":b["cnpg_uid"],"host":b["host"],"directPort":b["direct_port"],"pooledPort":b["pooled_port"],"state":b["state"],"members":members}));
        }
        tx.commit().await?;
        Ok(json!({"installations":result}))
    }
    pub async fn report_database_observations(
        &self,
        tx: DatabaseTransaction,
        agent: &AgentIdentity,
        data: Value,
    ) -> Result<Value> {
        use std::sync::OnceLock;
        static SCHEMA: OnceLock<jsonschema::Validator> = OnceLock::new();
        let schema=SCHEMA.get_or_init(||{let id=json!({"type":"string","pattern":"^[1-9][0-9]{0,18}$"});let bytes=json!({"type":"integer","minimum":0,"maximum":9007199254740991_u64});jsonschema::options().should_validate_formats(true).build(&json!({"type":"object","additionalProperties":false,"required":["observedAt","installations"],"properties":{"observedAt":{"type":"string","format":"date-time"},"installations":{"type":"array","maxItems":100,"items":{"type":"object","additionalProperties":false,"required":["installationID","cnpgUID","ready","filesystemBytes","databases"],"properties":{"installationID":id,"cnpgUID":{"type":"string","minLength":1,"maxLength":128},"ready":{"type":"boolean"},"filesystemBytes":{"anyOf":[bytes,{"type":"null"}]},"databases":{"type":"array","maxItems":100,"items":{"type":"object","additionalProperties":false,"required":["id","logicalBytes"],"properties":{"id":id,"logicalBytes":bytes}}}}}}}})).expect("database observation schema")});
        if !schema.is_valid(&data) {
            return Err(invalid());
        }
        let observed = chrono::DateTime::parse_from_rfc3339(text(&data, "observedAt")?)
            .map_err(|_| invalid())?
            .with_timezone(&chrono::Utc);
        if chrono::Utc::now()
            .signed_duration_since(observed)
            .num_milliseconds()
            .abs()
            > 30000
        {
            return Err(conflict("Database observations must be fresh."));
        }
        self.agent_scope(&tx, agent).await?;
        let capability=one(&tx,"SELECT jsonb_build_object('enabled',observation#>'{capabilities,databaseVersion}'='1'::jsonb) AS value FROM webdock_auth.hosting_agent WHERE cluster_id=$1",vec![agent.cluster_id().into()]).await?;
        if capability["enabled"] != true {
            return Err(conflict("A qualified database observer is required."));
        }
        for item in data["installations"].as_array().ok_or_else(invalid)? {
            let b=one(&tx,"SELECT to_jsonb(b) AS value FROM webdock_auth.hosting_database_backend b WHERE id=$1 AND cluster_id=$2 AND cnpg_uid=$3",vec![text(item,"installationID")?.into(),agent.cluster_id().into(),text(item,"cnpgUID")?.into()]).await?;
            let ready = item["ready"].as_bool().ok_or_else(invalid)?;
            let filesystem = item["filesystemBytes"].as_i64();
            exec(&tx,"UPDATE webdock_auth.hosting_database_backend SET observed_at=$2,filesystem_used_bytes=$3,state=CASE WHEN state IN('ready','degraded') THEN CASE WHEN $4 THEN 'ready' ELSE 'degraded' END ELSE state END WHERE id=$1 AND(observed_at IS NULL OR observed_at<=$2)",vec![text(&b,"id")?.into(),observed.into(),filesystem.into(),ready.into()]).await?;
            let mut ids = vec![];
            for d in item["databases"].as_array().ok_or_else(invalid)? {
                let id = text(d, "id")?;
                ids.push(id.to_owned());
                let usage = json!({"logicalBytes":d["logicalBytes"],"filesystemBytes":if b["kind"]=="dedicated"{item["filesystemBytes"].clone()}else{Value::Null},"observedAt":observed.to_rfc3339_opts(chrono::SecondsFormat::Millis,true)});
                exec(&tx,"UPDATE webdock_auth.hosting_database SET usage=$3,observed_at=$4,state=CASE WHEN state IN('ready','degraded') THEN CASE WHEN $5 THEN 'ready' ELSE 'degraded' END ELSE state END WHERE id=$1 AND backend_id=$2 AND state<>'purged' AND(observed_at IS NULL OR observed_at<=$4)",vec![id.into(),text(&b,"id")?.into(),usage.into(),observed.into(),ready.into()]).await?;
            }
            exec(&tx,"UPDATE webdock_auth.hosting_database SET state='degraded' WHERE backend_id=$1 AND state='ready' AND NOT(id=ANY($2::varchar[])) AND(observed_at IS NULL OR observed_at<=$3)",vec![text(&b,"id")?.into(),ids.into(),observed.into()]).await?;
            if !ready {
                exec(&tx,"UPDATE webdock_auth.hosting_database SET state='degraded' WHERE backend_id=$1 AND state='ready' AND(observed_at IS NULL OR observed_at<=$2)",vec![text(&b,"id")?.into(),observed.into()]).await?;
            }
        }
        tx.commit().await?;
        Ok(json!({"accepted":true}))
    }
}
