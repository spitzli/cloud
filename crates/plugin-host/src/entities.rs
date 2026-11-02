pub(crate) mod document {
    use sea_orm::entity::prelude::*;
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(schema_name = "webdock_auth", table_name = "plugin_document")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub plugin_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub customer_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub project_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub key: String,
        pub value: Json,
        pub revision: i64,
        pub updated_at: DateTimeWithTimeZone,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}
pub(crate) mod receipt {
    use sea_orm::entity::prelude::*;
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(schema_name = "webdock_auth", table_name = "plugin_document_receipt")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub plugin_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub customer_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub project_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub operation_id: String,
        #[sea_orm(primary_key, auto_increment = false)]
        pub idempotency_key: String,
        pub key: String,
        pub payload_sha256: String,
        pub result: Json,
        pub created_at: DateTimeWithTimeZone,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}
