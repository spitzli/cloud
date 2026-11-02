pub(crate) mod app {
    use sea_orm::entity::prelude::*;
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(schema_name = "webdock_auth", table_name = "hosting_app")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: String,
        pub project_id: String,
        pub name: String,
        pub spec: Json,
        pub previous_spec: Option<Json>,
        pub revision: i32,
        pub observed_revision: i32,
        pub status: String,
        pub operation_id: Option<String>,
        pub last_error: Option<String>,
        pub logs: Option<String>,
        pub observed_uid: Option<String>,
        pub created_at: DateTimeWithTimeZone,
        pub lifecycle: String,
        pub observed_spec: Option<Json>,
        pub environment_encrypted: Option<String>,
        pub previous_environment_encrypted: Option<String>,
        pub observed_environment_encrypted: Option<String>,
    }
    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}
    impl ActiveModelBehavior for ActiveModel {}
}
