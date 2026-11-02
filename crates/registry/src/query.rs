use crate::{RegistryError, Result, contracts::ListInput};
use sea_orm::sea_query::extension::postgres::PgExpr;
use sea_orm::{
    sea_query::{Expr, ExprTrait, SimpleExpr},
    *,
};
use serde::Serialize;
use serde_json::{Value as Json, json};

pub(crate) fn encode<T: Serialize>(model: T) -> Result<Json> {
    let mut value = serde_json::to_value(model)
        .map_err(|_| RegistryError::Invalid("Invalid registry record."))?;
    for key in ["createdAt", "updatedAt"] {
        if let Some(date) = value.get(key).and_then(Json::as_str) {
            let date = chrono::DateTime::parse_from_rfc3339(date)
                .map_err(|_| RegistryError::Invalid("Invalid registry timestamp."))?;
            value[key] = date
                .to_utc()
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                .into();
        }
    }
    Ok(value)
}
fn column<E: EntityTrait>(name: &str) -> Result<E::Column> {
    E::Column::iter()
        .find(|col| col.json_key() == name)
        .ok_or(RegistryError::Invalid("Invalid registry field."))
}
fn scalar(value: &Json) -> Result<Value> {
    match value {
        Json::Null => Ok(Value::String(None)),
        Json::String(value) if value.encode_utf16().count() <= 3000 => Ok(value.clone().into()),
        Json::Bool(value) => Ok(value.to_string().into()),
        Json::Number(value) => Ok(value
            .as_f64()
            .ok_or(RegistryError::Invalid("Invalid filter value."))?
            .to_string()
            .into()),
        _ => Err(RegistryError::Invalid("Invalid filter value.")),
    }
}
// node-postgres sends scalar filters as text; SQL casts retain timestamp/enum comparison semantics.
fn bound<C: ColumnTrait>(column: C, value: &Json) -> Result<Expr> {
    let value = Expr::val(scalar(value)?);
    Ok(match column.def().get_column_type() {
        ColumnType::TimestampWithTimeZone => {
            value.cast_as(sea_orm::sea_query::Alias::new("timestamptz"))
        }
        _ => column.save_as(value),
    })
}
fn escaped(value: &str) -> String {
    format!(
        "%{}%",
        value
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}
fn where_inner<E: EntityTrait>(value: &Json, depth: usize, nodes: &mut usize) -> Result<Condition> {
    *nodes += 1;
    let object = value
        .as_object()
        .filter(|o| o.len() <= 20)
        .ok_or(RegistryError::Invalid("Invalid registry filter."))?;
    if *nodes > 100 || depth > 6 {
        return Err(RegistryError::Invalid("Invalid registry filter."));
    }
    let mut result = Condition::all();
    for (field, operation) in object {
        if field == "and" || field == "or" {
            let children = operation
                .as_array()
                .filter(|a| a.len() <= 20)
                .ok_or(RegistryError::Invalid("Invalid registry filter."))?;
            let mut condition = if field == "and" {
                Condition::all()
            } else {
                Condition::any()
            };
            for child in children {
                let child = where_inner::<E>(child, depth + 1, nodes)?;
                if !child.is_empty() {
                    condition = condition.add(child);
                }
            }
            if condition.is_empty() {
                condition = condition.add(Expr::val(field == "and").eq(true));
            }
            result = result.add(condition);
            continue;
        }
        let col = column::<E>(field)?;
        let operation = operation
            .as_object()
            .filter(|o| o.len() == 1)
            .ok_or(RegistryError::Invalid("Invalid registry filter."))?;
        let (operator, value) = operation.iter().next().unwrap();
        let expression: SimpleExpr = match operator.as_str() {
            "exists" if value.is_boolean() => {
                if value.as_bool().unwrap() {
                    col.is_not_null()
                } else {
                    col.is_null()
                }
            }
            "in" | "not_in" if value.is_array() => {
                let values = value.as_array().unwrap();
                if values.len() > 100 {
                    return Err(RegistryError::Invalid("Invalid registry filter."));
                }
                let values = values
                    .iter()
                    .map(|value| bound(col, value))
                    .collect::<Result<Vec<_>>>()?;
                if operator == "in" {
                    Expr::col(col).is_in(values)
                } else {
                    Expr::col(col).is_not_in(values)
                }
            }
            "equals" => {
                if value.is_null() {
                    col.is_null()
                } else {
                    Expr::col(col).eq(bound(col, value)?)
                }
            }
            "not_equals" => {
                if value.is_null() {
                    col.is_not_null()
                } else {
                    Expr::col(col).ne(bound(col, value)?)
                }
            }
            "contains" if value.is_string() => {
                scalar(value)?;
                Expr::col(col).ilike(escaped(value.as_str().unwrap()))
            }
            _ => return Err(RegistryError::Invalid("Unsupported registry filter.")),
        };
        result = result.add(expression);
    }
    Ok(result)
}
pub(crate) fn filter<E: EntityTrait>(value: Option<&Json>) -> Result<Condition> {
    match value {
        Some(value) => where_inner::<E>(value, 0, &mut 0),
        None => Ok(Condition::all()),
    }
}
pub(crate) async fn list<E: EntityTrait>(
    tx: &DatabaseTransaction,
    input: &ListInput,
) -> Result<Json>
where
    E::Model: Serialize + Send + Sync,
{
    let mut condition = Condition::all();
    for (key, value) in [
        ("customer", &input.customer),
        ("targetID", &input.target_id),
        ("targetCollection", &input.target_collection),
    ] {
        if let Some(value) = value {
            let allowed = match input.collection.as_str() {
                "projects" => key == "customer",
                "audit-events" => key == "targetID" || key == "targetCollection",
                _ => false,
            };
            if !allowed {
                return Err(RegistryError::Invalid("Invalid collection filter."));
            }
            condition = condition.add(column::<E>(key)?.eq(value.clone()));
        }
    }
    if let Some(status) = &input.status
        && !status.is_empty()
    {
        if input.collection == "audit-events" || !["active", "archived"].contains(&status.as_str())
        {
            return Err(RegistryError::Invalid("Invalid status filter."));
        }
        condition = condition.add(column::<E>("status")?.eq(status.clone()));
    }
    if let Some(search) = &input.search
        && !search.is_empty()
    {
        condition = condition.add(
            Expr::col(column::<E>(if input.collection == "audit-events" {
                "summary"
            } else {
                "name"
            })?)
            .ilike(escaped(search)),
        );
    }
    let field = input.sort.strip_prefix('-').unwrap_or(&input.sort);
    if !["createdAt", "updatedAt", "name"].contains(&field) {
        return Err(RegistryError::Invalid("Invalid sort."));
    }
    let sort = column::<E>(field)?;
    let total = E::find().filter(condition.clone()).count(tx).await?;
    let docs = E::find()
        .filter(condition)
        .order_by(
            sort,
            if input.sort.starts_with('-') {
                Order::Desc
            } else {
                Order::Asc
            },
        )
        .order_by_asc(column::<E>("id")?)
        .limit(input.limit)
        .offset((input.page - 1) * input.limit)
        .all(tx)
        .await?;
    let docs = docs.into_iter().map(encode).collect::<Result<Vec<_>>>()?;
    let pages = std::cmp::max(total.div_ceil(input.limit), 1);
    Ok(
        json!({"docs":docs,"page":input.page,"totalPages":pages,"totalDocs":total,"hasNextPage":input.page<pages,"hasPrevPage":input.page>1}),
    )
}
pub(crate) async fn query<E: EntityTrait>(
    tx: &DatabaseTransaction,
    mode: &str,
    raw: &Json,
) -> Result<Json>
where
    E::Model: Serialize + Send + Sync,
{
    let page = number(raw, "page", 1, 10000)?;
    let limit = number(raw, "limit", 20, 1000)?;
    let mut condition = filter::<E>(raw.get("where"))?;
    if mode == "findByID" {
        let id = raw
            .get("id")
            .and_then(Json::as_str)
            .ok_or(RegistryError::Invalid("Invalid ID."))?;
        crate::contracts::record_id(id)?;
        condition = condition.add(column::<E>("id")?.eq(id));
    }
    if mode == "count" {
        return Ok(json!({"totalDocs":E::find().filter(condition).count(tx).await?}));
    }
    let sorts = match raw.get("sort") {
        Some(Json::Array(values)) => values.clone(),
        Some(Json::String(value)) if !value.is_empty() => vec![Json::String(value.clone())],
        None => {
            vec![Json::String("-updatedAt".into())]
        }
        Some(value) if !truthy(value) => vec![Json::String("-updatedAt".into())],
        _ => return Err(RegistryError::Invalid("Invalid sort.")),
    };
    if sorts.len() > 3 {
        return Err(RegistryError::Invalid("Invalid sort."));
    }
    let mut select = E::find().filter(condition.clone());
    for sort in sorts {
        let sort = sort
            .as_str()
            .ok_or(RegistryError::Invalid("Invalid sort."))?;
        select = select.order_by(
            column::<E>(sort.strip_prefix('-').unwrap_or(sort))?,
            if sort.starts_with('-') {
                Order::Desc
            } else {
                Order::Asc
            },
        );
    }
    let selection = if let Some(value) = raw.get("select") {
        let selection = value
            .as_object()
            .ok_or(RegistryError::Invalid("Invalid selection."))?;
        for (field, selected) in selection {
            column::<E>(field)?;
            if !selected.is_boolean() {
                return Err(RegistryError::Invalid("Invalid selection."));
            }
        }
        Some(selection)
    } else {
        None
    };
    let docs = select
        .order_by_asc(column::<E>("id")?)
        .limit(if mode == "findByID" { 1 } else { limit })
        .offset(if mode == "findByID" {
            0
        } else {
            (page - 1) * limit
        })
        .all(tx)
        .await?;
    let mut docs = docs.into_iter().map(encode).collect::<Result<Vec<_>>>()?;
    if let Some(selection) = selection {
        let include = selection.values().any(|v| v == true);
        for doc in &mut docs {
            doc.as_object_mut().unwrap().retain(|key, _| {
                key == "id"
                    || if include {
                        selection.get(key) == Some(&Json::Bool(true))
                    } else {
                        selection.get(key) != Some(&Json::Bool(false))
                    }
            });
        }
    }
    if mode == "findByID" {
        return match docs.into_iter().next() {
            Some(doc) => Ok(doc),
            None if raw.get("disableErrors").is_some_and(truthy) => Ok(Json::Null),
            None => Err(RegistryError::NotFound("Record unavailable.")),
        };
    }
    let total = E::find().filter(condition).count(tx).await?;
    let pages = std::cmp::max(total.div_ceil(limit), 1);
    Ok(
        json!({"docs":docs,"totalDocs":total,"totalPages":pages,"page":page,"limit":limit,"hasNextPage":page<pages,"hasPrevPage":page>1,"nextPage":if page<pages {Some(page+1)}else{None},"prevPage":if page>1{Some(page-1)}else{None}}),
    )
}
fn truthy(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(v) => *v,
        Json::Number(v) => v.as_f64() != Some(0.0),
        Json::String(v) => !v.is_empty(),
        _ => true,
    }
}
pub(crate) fn number(raw: &Json, key: &str, default: u64, max: u64) -> Result<u64> {
    let value = match raw.get(key) {
        None => default,
        Some(value) => {
            let number =
                javascript_number(value).ok_or(RegistryError::Invalid("Invalid pagination."))?;
            if !number.is_finite() || number.fract() != 0.0 || number < 0.0 || number > max as f64 {
                return Err(RegistryError::Invalid("Invalid pagination."));
            }
            number as u64
        }
    };
    if value > max || (key != "depth" && value == 0) {
        return Err(RegistryError::Invalid("Invalid pagination."));
    }
    Ok(value)
}
fn javascript_number(value: &Json) -> Option<f64> {
    fn array_string(value: &Json) -> String {
        match value {
            Json::Null => String::new(),
            Json::String(value) => value.clone(),
            Json::Array(values) => values
                .iter()
                .map(array_string)
                .collect::<Vec<_>>()
                .join(","),
            Json::Object(_) => "[object Object]".into(),
            value => value.to_string(),
        }
    }
    match value {
        Json::Null => Some(0.0),
        Json::Bool(value) => Some(if *value { 1.0 } else { 0.0 }),
        Json::Number(value) => value.as_f64(),
        Json::Object(_) => None,
        value => {
            let raw = array_string(value);
            let raw = crate::contracts::trim(&raw);
            if raw.is_empty() {
                return Some(0.0);
            }
            for (prefix, radix) in [
                ("0x", 16),
                ("0X", 16),
                ("0b", 2),
                ("0B", 2),
                ("0o", 8),
                ("0O", 8),
            ] {
                if let Some(digits) = raw.strip_prefix(prefix) {
                    return u64::from_str_radix(digits, radix).ok().map(|v| v as f64);
                }
            }
            raw.parse().ok()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::customer;
    #[test]
    fn bounded_filter_rejects_unknown_fields_and_operators() {
        for input in [
            json!({"password":{"equals":"secret"}}),
            json!({"name":{"like":"%"}}),
            json!({"name":{"equals":[],"contains":"x"}}),
            json!({"and":"bad"}),
        ] {
            assert!(filter::<customer::Entity>(Some(&input)).is_err());
        }
        let mut deep = json!({"name":{"equals":"x"}});
        for _ in 0..7 {
            deep = json!({"and":[deep]});
        }
        assert!(filter::<customer::Entity>(Some(&deep)).is_err());
        let sql = customer::Entity::find()
            .filter(
                filter::<customer::Entity>(Some(&json!({"name":{"contains":"%_\\'"}}))).unwrap(),
            )
            .build(DbBackend::Postgres);
        assert!(sql.sql.as_str().contains("ILIKE $1"));
        assert_eq!(escaped("%_\\'"), "%\\%\\_\\\\'%");
    }
}
