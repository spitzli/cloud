use super::*;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(schema_name = "webdock_admin", table_name = "audit_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[serde(rename = "id")]
    pub id: String,
    #[sea_orm(column_name = "actor_id")]
    #[serde(rename = "actor")]
    pub actor: String,
    #[serde(rename = "action")]
    pub action: AuditAction,
    #[serde(rename = "targetCollection")]
    pub target_collection: String,
    #[sea_orm(column_name = "target_i_d")]
    #[serde(rename = "targetID")]
    pub target_id: String,
    #[serde(rename = "summary")]
    pub summary: String,
    #[serde(rename = "changedFields")]
    pub changed_fields: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTimeWithTimeZone,
    #[serde(rename = "createdAt")]
    pub created_at: DateTimeWithTimeZone,
}
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}
