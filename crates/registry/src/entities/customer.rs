use super::*;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(schema_name = "webdock_admin", table_name = "customers")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "contactName")]
    pub contact_name: Option<String>,
    #[serde(rename = "contactEmail")]
    pub contact_email: Option<String>,
    #[serde(rename = "notes")]
    pub notes: Option<String>,
    #[serde(rename = "status")]
    pub status: CustomerStatus,
    #[serde(rename = "updatedAt")]
    pub updated_at: DateTimeWithTimeZone,
    #[serde(rename = "createdAt")]
    pub created_at: DateTimeWithTimeZone,
    #[serde(rename = "customerType")]
    pub customer_type: Option<CustomerType>,
    #[serde(rename = "firstName")]
    pub first_name: Option<String>,
    #[serde(rename = "lastName")]
    pub last_name: Option<String>,
    #[serde(rename = "companyName")]
    pub company_name: Option<String>,
    #[serde(rename = "phone")]
    pub phone: Option<String>,
    #[serde(rename = "addressLine1")]
    pub address_line1: Option<String>,
    #[serde(rename = "addressLine2")]
    pub address_line2: Option<String>,
    #[serde(rename = "postalCode")]
    pub postal_code: Option<String>,
    #[serde(rename = "city")]
    pub city: Option<String>,
    #[serde(rename = "region")]
    pub region: Option<String>,
    #[serde(rename = "country")]
    pub country: Option<String>,
}
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}
impl ActiveModelBehavior for ActiveModel {}
