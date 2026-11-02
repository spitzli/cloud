use super::*;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(schema_name = "webdock_admin", table_name = "projects")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[sea_orm(column_name = "customer_id")]
    #[serde(rename = "customer")]
    pub customer: String,
    #[serde(rename = "url")]
    pub url: Option<String>,
    #[sea_orm(column_name = "repository_u_r_l")]
    #[serde(rename = "repositoryURL")]
    pub repository_url: Option<String>,
    #[serde(rename = "notes")]
    pub notes: Option<String>,
    #[serde(rename = "status")]
    pub status: ProjectStatus,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTimeWithTimeZone,
    #[serde(rename = "createdAt")]
    pub created_at: DateTimeWithTimeZone,
}
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}
