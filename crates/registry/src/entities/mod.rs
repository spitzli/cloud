pub mod audit;
pub mod customer;
pub mod project;
pub mod user;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(
    rs_type = "String",
    db_type = "Enum",
    enum_name = "enum_customers_status"
)]
#[serde(rename_all = "lowercase")]
pub enum CustomerStatus {
    #[sea_orm(string_value = "active")]
    Active,
    #[sea_orm(string_value = "archived")]
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(
    rs_type = "String",
    db_type = "Enum",
    enum_name = "enum_projects_status"
)]
#[serde(rename_all = "lowercase")]
pub enum ProjectStatus {
    #[sea_orm(string_value = "active")]
    Active,
    #[sea_orm(string_value = "archived")]
    Archived,
}

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(
    rs_type = "String",
    db_type = "Enum",
    enum_name = "enum_customers_customer_type"
)]
#[serde(rename_all = "lowercase")]
pub enum CustomerType {
    #[sea_orm(string_value = "person")]
    Person,
    #[sea_orm(string_value = "company")]
    Company,
}

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(
    rs_type = "String",
    db_type = "Enum",
    enum_name = "enum_audit_events_action"
)]
#[serde(rename_all = "lowercase")]
pub enum AuditAction {
    #[sea_orm(string_value = "create")]
    Create,
    #[sea_orm(string_value = "update")]
    Update,
    #[sea_orm(string_value = "delete")]
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, EnumIter, DeriveActiveEnum, Serialize, Deserialize)]
#[sea_orm(rs_type = "String", db_type = "Enum", enum_name = "enum_users_role")]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    #[sea_orm(string_value = "operator")]
    Operator,
    #[sea_orm(string_value = "admin")]
    Admin,
    #[sea_orm(string_value = "editor")]
    Editor,
    #[sea_orm(string_value = "reader")]
    Reader,
}
