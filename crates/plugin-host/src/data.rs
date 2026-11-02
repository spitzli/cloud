use crate::{
    HostService, InvocationHandle, conflict, db_error,
    entities::{document, receipt},
    invalid, unavailable,
};
use icu_collator::{Collator, options::CollatorOptions};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DbBackend, EntityTrait, IntoActiveModel, QuerySelect, Set,
    Statement,
};
use serde_json::{Value as Json, json};
use webdock_plugin_protocol::Result;

pub fn validate_key(key: &str) -> Result<()> {
    if key.is_empty()
        || key.len() > 240
        || !key.as_bytes()[0].is_ascii_alphanumeric()
        || !key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
    {
        return Err(invalid("Invalid document key"));
    }
    Ok(())
}
pub fn validate_write(value: Json) -> Result<Json> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("Invalid document write"))?;
    if object.len() != 4
        || !["key", "value", "expectedRevision", "idempotencyKey"]
            .iter()
            .all(|key| object.contains_key(*key))
    {
        return Err(invalid("Invalid document write"));
    }
    validate_key(
        value["key"]
            .as_str()
            .ok_or_else(|| invalid("Invalid document key"))?,
    )?;
    if !value["expectedRevision"].is_null()
        && !value["expectedRevision"].as_str().is_some_and(revision)
    {
        return Err(invalid("Invalid document revision"));
    }
    let key = value["idempotencyKey"]
        .as_str()
        .ok_or_else(|| invalid("Invalid idempotency key"))?;
    if !(8..=128).contains(&key.len())
        || !key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
    {
        return Err(invalid("Invalid idempotency key"));
    }
    if javascript_json(&value["value"], false)?.len() > 65536 {
        return Err(invalid("Document exceeds 64 KiB"));
    }
    legacy_json_value(&value)
}
/// Normalize values at the retained JavaScript v1 boundary, so stored values and receipt
/// hashes describe the same numbers and integer-key enumeration order.
pub fn legacy_json_value(value: &Json) -> Result<Json> {
    serde_json::from_str(&javascript_json(value, false)?)
        .map_err(|_| invalid("Invalid legacy JSON value"))
}
pub(crate) fn revision(value: &str) -> bool {
    value == "0"
        || (!value.starts_with('0')
            && !value.is_empty()
            && value.len() <= 19
            && value.bytes().all(|c| c.is_ascii_digit()))
}
/// Retained v1 hash encoding: en-US ICU localeCompare, ECMAScript property order and numbers.
/// Numeric property names sort numerically even after Object.fromEntries(locale-sorted entries).
pub fn javascript_json(value: &Json, canonical: bool) -> Result<String> {
    let collator = Collator::try_new(
        icu_locale::locale!("en-US").into(),
        CollatorOptions::default(),
    )
    .map_err(|_| unavailable())?;
    fn index(key: &str) -> Option<u32> {
        let n = key.parse::<u32>().ok()?;
        if n == u32::MAX || n.to_string() != key {
            None
        } else {
            Some(n)
        }
    }
    fn encode(
        value: &Json,
        canonical: bool,
        collator: &icu_collator::CollatorBorrowed<'_>,
        out: &mut String,
        depth: usize,
    ) -> Result<()> {
        if depth > 64 {
            return Err(invalid("Document nesting exceeds limit"));
        }
        match value {
            Json::Null => out.push_str("null"),
            Json::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
            Json::Number(v) => {
                let n = v.as_f64().ok_or_else(|| invalid("Invalid JSON number"))?;
                out.push_str(ryu_js::Buffer::new().format_finite(n));
            }
            Json::String(v) => {
                out.push_str(&serde_json::to_string(v).map_err(|_| invalid("Invalid JSON string"))?)
            }
            Json::Array(values) => {
                out.push('[');
                for (i, value) in values.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    encode(value, canonical, collator, out, depth + 1)?;
                }
                out.push(']');
            }
            Json::Object(values) => {
                let mut fields = values.iter().collect::<Vec<_>>();
                if canonical {
                    fields.sort_by(|a, b| collator.compare(a.0, b.0));
                }
                fields.sort_by(|a, b| match (index(a.0), index(b.0)) {
                    (Some(a), Some(b)) => a.cmp(&b),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                });
                out.push('{');
                for (i, (key, value)) in fields.into_iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(
                        &serde_json::to_string(key).map_err(|_| invalid("Invalid JSON key"))?,
                    );
                    out.push(':');
                    encode(value, canonical, collator, out, depth + 1)?;
                }
                out.push('}');
            }
        }
        Ok(())
    }
    let mut output = String::new();
    encode(value, canonical, &collator, &mut output, 0)?;
    Ok(output)
}
impl HostService {
    pub async fn data_read(&self, handle: &InvocationHandle, key: &str) -> Result<Json> {
        validate_key(key)?;
        let context = self.authorized(handle, Some("plugin.data.read")).await?;
        let scope = &context.scope;
        let record = document::Entity::find_by_id((
            scope.plugin_id.clone(),
            scope.customer_id.clone(),
            scope.project_id.clone(),
            key.to_owned(),
        ))
        .one(&context.tx)
        .await
        .map_err(db_error)?;
        let result = record.map_or(
            json!({"value":null,"revision":null}),
            |record| json!({"value":record.value,"revision":record.revision.to_string()}),
        );
        self.finish(context, handle, Some("plugin.data.read"))
            .await?;
        Ok(result)
    }
    pub async fn data_write(&self, handle: &InvocationHandle, raw: Json) -> Result<Json> {
        let input = validate_write(raw)?;
        let context = self.authorized(handle, Some("plugin.data.write")).await?;
        let scope = &context.scope;
        let key = input["key"].as_str().unwrap();
        let idempotency = input["idempotencyKey"].as_str().unwrap();
        let digest = crate::authority::digest(javascript_json(&input, true)?);
        for lock in [
            json!([scope.plugin_id, scope.customer_id, scope.project_id, key]),
            json!([
                scope.plugin_id,
                scope.customer_id,
                scope.project_id,
                scope.operation_id,
                idempotency
            ]),
        ] {
            context
                .tx
                .query_one_raw(Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
                    [lock.to_string().into()],
                ))
                .await
                .map_err(db_error)?;
        }
        let receipt_key = (
            scope.plugin_id.clone(),
            scope.customer_id.clone(),
            scope.project_id.clone(),
            scope.operation_id.clone(),
            idempotency.to_owned(),
        );
        if let Some(prior) = receipt::Entity::find_by_id(receipt_key)
            .one(&context.tx)
            .await
            .map_err(db_error)?
        {
            if prior.payload_sha256 != digest {
                return Err(conflict("Idempotency key reused with different data"));
            }
            self.finish(context, handle, Some("plugin.data.write"))
                .await?;
            return Ok(prior.result);
        }
        let primary = (
            scope.plugin_id.clone(),
            scope.customer_id.clone(),
            scope.project_id.clone(),
            key.to_owned(),
        );
        let current = document::Entity::find_by_id(primary)
            .lock_exclusive()
            .one(&context.tx)
            .await
            .map_err(db_error)?;
        if current.as_ref().map(|c| c.revision.to_string()).as_deref()
            != input["expectedRevision"].as_str()
        {
            return Err(conflict("Document changed; reload before saving"));
        }
        let now = chrono::Utc::now().into();
        let record = if let Some(current) = current {
            let revision = current
                .revision
                .checked_add(1)
                .ok_or_else(|| conflict("Document revision exhausted"))?;
            let mut active = current.into_active_model();
            active.value = Set(input["value"].clone());
            active.revision = Set(revision);
            active.updated_at = Set(now);
            active.update(&context.tx).await.map_err(db_error)?
        } else {
            document::ActiveModel {
                plugin_id: Set(scope.plugin_id.clone()),
                customer_id: Set(scope.customer_id.clone()),
                project_id: Set(scope.project_id.clone()),
                key: Set(key.into()),
                value: Set(input["value"].clone()),
                revision: Set(1),
                updated_at: Set(now),
            }
            .insert(&context.tx)
            .await
            .map_err(db_error)?
        };
        let result = json!({"value":record.value,"revision":record.revision.to_string()});
        receipt::ActiveModel {
            plugin_id: Set(scope.plugin_id.clone()),
            customer_id: Set(scope.customer_id.clone()),
            project_id: Set(scope.project_id.clone()),
            operation_id: Set(scope.operation_id.clone()),
            idempotency_key: Set(idempotency.into()),
            key: Set(key.into()),
            payload_sha256: Set(digest),
            result: Set(result.clone()),
            created_at: Set(now),
        }
        .insert(&context.tx)
        .await
        .map_err(db_error)?;
        self.finish(context, handle, Some("plugin.data.write"))
            .await?;
        Ok(result)
    }
}
