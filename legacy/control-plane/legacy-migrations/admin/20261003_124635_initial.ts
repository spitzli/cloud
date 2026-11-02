import { snowflakeSQL } from "./snowflake-schema";
import { MigrateUpArgs, MigrateDownArgs, sql } from "@payloadcms/db-postgres";

export async function up({ db }: MigrateUpArgs): Promise<void> {
  await db.execute(sql.raw(snowflakeSQL));
  await db.execute(sql`
   CREATE TYPE "webdock_admin"."enum_users_role" AS ENUM('operator', 'admin', 'editor', 'reader');
  CREATE TYPE "webdock_admin"."enum_customers_status" AS ENUM('active', 'archived');
  CREATE TYPE "webdock_admin"."enum_projects_status" AS ENUM('active', 'archived');
  CREATE TYPE "webdock_admin"."enum_cms_instances_provider" AS ENUM('vercel', 'other');
  CREATE TYPE "webdock_admin"."enum_cms_instances_template" AS ENUM('webdock-landing', 'spitzli-portfolio', 'stall-business', 'custom');
  CREATE TYPE "webdock_admin"."enum_cms_instances_status" AS ENUM('active', 'suspended', 'retired');
  CREATE TYPE "webdock_admin"."enum_audit_events_action" AS ENUM('create', 'update');
  CREATE TABLE "webdock_admin"."users_sessions" (
    "_order" integer NOT NULL,
    "_parent_id" varchar NOT NULL,
    "id" varchar PRIMARY KEY NOT NULL,
    "created_at" timestamp(3) with time zone,
    "expires_at" timestamp(3) with time zone NOT NULL
  );

  CREATE TABLE "webdock_admin"."users" (
    "id" varchar PRIMARY KEY NOT NULL,
    "name" varchar NOT NULL,
    "role" "webdock_admin"."enum_users_role" DEFAULT 'editor' NOT NULL,
    "updated_at" timestamp(3) with time zone DEFAULT now() NOT NULL,
    "created_at" timestamp(3) with time zone DEFAULT now() NOT NULL,
    "email" varchar NOT NULL,
    "reset_password_token" varchar,
    "reset_password_expiration" timestamp(3) with time zone,
    "salt" varchar,
    "hash" varchar,
    "reset_password_requested_at" timestamp(3) with time zone,
    "login_attempts" numeric DEFAULT 0,
    "lock_until" timestamp(3) with time zone
  );

  CREATE TABLE "webdock_admin"."customers" (
    "id" varchar PRIMARY KEY NOT NULL,
    "name" varchar NOT NULL,
    "contact_name" varchar,
    "contact_email" varchar,
    "notes" varchar,
    "status" "webdock_admin"."enum_customers_status" DEFAULT 'active' NOT NULL,
    "updated_at" timestamp(3) with time zone DEFAULT now() NOT NULL,
    "created_at" timestamp(3) with time zone DEFAULT now() NOT NULL
  );

  CREATE TABLE "webdock_admin"."projects" (
    "id" varchar PRIMARY KEY NOT NULL,
    "name" varchar NOT NULL,
    "customer_id" varchar NOT NULL,
    "url" varchar,
    "repository_u_r_l" varchar,
    "notes" varchar,
    "status" "webdock_admin"."enum_projects_status" DEFAULT 'active' NOT NULL,
    "updated_at" timestamp(3) with time zone DEFAULT now() NOT NULL,
    "created_at" timestamp(3) with time zone DEFAULT now() NOT NULL
  );

  CREATE TABLE "webdock_admin"."cms_instances" (
    "id" varchar PRIMARY KEY NOT NULL,
    "label" varchar NOT NULL,
    "project_id" varchar NOT NULL,
    "admin_u_r_l" varchar NOT NULL,
    "schema_name" varchar NOT NULL,
    "provider" "webdock_admin"."enum_cms_instances_provider" DEFAULT 'vercel' NOT NULL,
    "provider_project_i_d" varchar NOT NULL,
    "template" "webdock_admin"."enum_cms_instances_template" NOT NULL,
    "payload_version" varchar,
    "status" "webdock_admin"."enum_cms_instances_status" DEFAULT 'active' NOT NULL,
    "notes" varchar,
    "updated_at" timestamp(3) with time zone DEFAULT now() NOT NULL,
    "created_at" timestamp(3) with time zone DEFAULT now() NOT NULL
  );

  CREATE TABLE "webdock_admin"."audit_events" (
    "id" varchar PRIMARY KEY NOT NULL,
    "actor_id" varchar NOT NULL,
    "action" "webdock_admin"."enum_audit_events_action" NOT NULL,
    "target_collection" varchar NOT NULL,
    "target_i_d" varchar NOT NULL,
    "summary" varchar NOT NULL,
    "changed_fields" varchar,
    "updated_at" timestamp(3) with time zone DEFAULT now() NOT NULL,
    "created_at" timestamp(3) with time zone DEFAULT now() NOT NULL
  );

  CREATE TABLE "webdock_admin"."payload_kv" (
    "id" serial PRIMARY KEY NOT NULL,
    "key" varchar NOT NULL,
    "data" jsonb NOT NULL
  );

  CREATE TABLE "webdock_admin"."payload_locked_documents" (
    "id" serial PRIMARY KEY NOT NULL,
    "global_slug" varchar,
    "updated_at" timestamp(3) with time zone DEFAULT now() NOT NULL,
    "created_at" timestamp(3) with time zone DEFAULT now() NOT NULL
  );

  CREATE TABLE "webdock_admin"."payload_locked_documents_rels" (
    "id" serial PRIMARY KEY NOT NULL,
    "order" integer,
    "parent_id" integer NOT NULL,
    "path" varchar NOT NULL,
    "users_id" varchar,
    "customers_id" varchar,
    "projects_id" varchar,
    "cms_instances_id" varchar,
    "audit_events_id" varchar
  );

  CREATE TABLE "webdock_admin"."payload_preferences" (
    "id" serial PRIMARY KEY NOT NULL,
    "key" varchar,
    "value" jsonb,
    "updated_at" timestamp(3) with time zone DEFAULT now() NOT NULL,
    "created_at" timestamp(3) with time zone DEFAULT now() NOT NULL
  );

  CREATE TABLE "webdock_admin"."payload_preferences_rels" (
    "id" serial PRIMARY KEY NOT NULL,
    "order" integer,
    "parent_id" integer NOT NULL,
    "path" varchar NOT NULL,
    "users_id" varchar
  );

  CREATE TABLE "webdock_admin"."payload_migrations" (
    "id" serial PRIMARY KEY NOT NULL,
    "name" varchar,
    "batch" numeric,
    "updated_at" timestamp(3) with time zone DEFAULT now() NOT NULL,
    "created_at" timestamp(3) with time zone DEFAULT now() NOT NULL
  );

  ALTER TABLE "webdock_admin"."users_sessions" ADD CONSTRAINT "users_sessions_parent_id_fk" FOREIGN KEY ("_parent_id") REFERENCES "webdock_admin"."users"("id") ON DELETE cascade ON UPDATE no action;
  ALTER TABLE "webdock_admin"."projects" ADD CONSTRAINT "projects_customer_id_customers_id_fk" FOREIGN KEY ("customer_id") REFERENCES "webdock_admin"."customers"("id") ON DELETE set null ON UPDATE no action;
  ALTER TABLE "webdock_admin"."cms_instances" ADD CONSTRAINT "cms_instances_project_id_projects_id_fk" FOREIGN KEY ("project_id") REFERENCES "webdock_admin"."projects"("id") ON DELETE set null ON UPDATE no action;
  ALTER TABLE "webdock_admin"."audit_events" ADD CONSTRAINT "audit_events_actor_id_users_id_fk" FOREIGN KEY ("actor_id") REFERENCES "webdock_admin"."users"("id") ON DELETE set null ON UPDATE no action;
  ALTER TABLE "webdock_admin"."payload_locked_documents_rels" ADD CONSTRAINT "payload_locked_documents_rels_parent_fk" FOREIGN KEY ("parent_id") REFERENCES "webdock_admin"."payload_locked_documents"("id") ON DELETE cascade ON UPDATE no action;
  ALTER TABLE "webdock_admin"."payload_locked_documents_rels" ADD CONSTRAINT "payload_locked_documents_rels_users_fk" FOREIGN KEY ("users_id") REFERENCES "webdock_admin"."users"("id") ON DELETE cascade ON UPDATE no action;
  ALTER TABLE "webdock_admin"."payload_locked_documents_rels" ADD CONSTRAINT "payload_locked_documents_rels_customers_fk" FOREIGN KEY ("customers_id") REFERENCES "webdock_admin"."customers"("id") ON DELETE cascade ON UPDATE no action;
  ALTER TABLE "webdock_admin"."payload_locked_documents_rels" ADD CONSTRAINT "payload_locked_documents_rels_projects_fk" FOREIGN KEY ("projects_id") REFERENCES "webdock_admin"."projects"("id") ON DELETE cascade ON UPDATE no action;
  ALTER TABLE "webdock_admin"."payload_locked_documents_rels" ADD CONSTRAINT "payload_locked_documents_rels_cms_instances_fk" FOREIGN KEY ("cms_instances_id") REFERENCES "webdock_admin"."cms_instances"("id") ON DELETE cascade ON UPDATE no action;
  ALTER TABLE "webdock_admin"."payload_locked_documents_rels" ADD CONSTRAINT "payload_locked_documents_rels_audit_events_fk" FOREIGN KEY ("audit_events_id") REFERENCES "webdock_admin"."audit_events"("id") ON DELETE cascade ON UPDATE no action;
  ALTER TABLE "webdock_admin"."payload_preferences_rels" ADD CONSTRAINT "payload_preferences_rels_parent_fk" FOREIGN KEY ("parent_id") REFERENCES "webdock_admin"."payload_preferences"("id") ON DELETE cascade ON UPDATE no action;
  ALTER TABLE "webdock_admin"."payload_preferences_rels" ADD CONSTRAINT "payload_preferences_rels_users_fk" FOREIGN KEY ("users_id") REFERENCES "webdock_admin"."users"("id") ON DELETE cascade ON UPDATE no action;
  CREATE INDEX "users_sessions_order_idx" ON "webdock_admin"."users_sessions" USING btree ("_order");
  CREATE INDEX "users_sessions_parent_id_idx" ON "webdock_admin"."users_sessions" USING btree ("_parent_id");
  CREATE INDEX "users_updated_at_idx" ON "webdock_admin"."users" USING btree ("updated_at");
  CREATE INDEX "users_created_at_idx" ON "webdock_admin"."users" USING btree ("created_at");
  CREATE UNIQUE INDEX "users_email_idx" ON "webdock_admin"."users" USING btree ("email");
  CREATE INDEX "customers_updated_at_idx" ON "webdock_admin"."customers" USING btree ("updated_at");
  CREATE INDEX "customers_created_at_idx" ON "webdock_admin"."customers" USING btree ("created_at");
  CREATE INDEX "projects_customer_idx" ON "webdock_admin"."projects" USING btree ("customer_id");
  CREATE INDEX "projects_updated_at_idx" ON "webdock_admin"."projects" USING btree ("updated_at");
  CREATE INDEX "projects_created_at_idx" ON "webdock_admin"."projects" USING btree ("created_at");
  CREATE UNIQUE INDEX "cms_instances_project_idx" ON "webdock_admin"."cms_instances" USING btree ("project_id");
  CREATE UNIQUE INDEX "cms_instances_schema_name_idx" ON "webdock_admin"."cms_instances" USING btree ("schema_name");
  CREATE UNIQUE INDEX "cms_instances_provider_project_i_d_idx" ON "webdock_admin"."cms_instances" USING btree ("provider_project_i_d");
  CREATE INDEX "cms_instances_updated_at_idx" ON "webdock_admin"."cms_instances" USING btree ("updated_at");
  CREATE INDEX "cms_instances_created_at_idx" ON "webdock_admin"."cms_instances" USING btree ("created_at");
  CREATE INDEX "audit_events_actor_idx" ON "webdock_admin"."audit_events" USING btree ("actor_id");
  CREATE INDEX "audit_events_target_i_d_idx" ON "webdock_admin"."audit_events" USING btree ("target_i_d");
  CREATE INDEX "audit_events_updated_at_idx" ON "webdock_admin"."audit_events" USING btree ("updated_at");
  CREATE INDEX "audit_events_created_at_idx" ON "webdock_admin"."audit_events" USING btree ("created_at");
  CREATE UNIQUE INDEX "payload_kv_key_idx" ON "webdock_admin"."payload_kv" USING btree ("key");
  CREATE INDEX "payload_locked_documents_global_slug_idx" ON "webdock_admin"."payload_locked_documents" USING btree ("global_slug");
  CREATE INDEX "payload_locked_documents_updated_at_idx" ON "webdock_admin"."payload_locked_documents" USING btree ("updated_at");
  CREATE INDEX "payload_locked_documents_created_at_idx" ON "webdock_admin"."payload_locked_documents" USING btree ("created_at");
  CREATE INDEX "payload_locked_documents_rels_order_idx" ON "webdock_admin"."payload_locked_documents_rels" USING btree ("order");
  CREATE INDEX "payload_locked_documents_rels_parent_idx" ON "webdock_admin"."payload_locked_documents_rels" USING btree ("parent_id");
  CREATE INDEX "payload_locked_documents_rels_path_idx" ON "webdock_admin"."payload_locked_documents_rels" USING btree ("path");
  CREATE INDEX "payload_locked_documents_rels_users_id_idx" ON "webdock_admin"."payload_locked_documents_rels" USING btree ("users_id");
  CREATE INDEX "payload_locked_documents_rels_customers_id_idx" ON "webdock_admin"."payload_locked_documents_rels" USING btree ("customers_id");
  CREATE INDEX "payload_locked_documents_rels_projects_id_idx" ON "webdock_admin"."payload_locked_documents_rels" USING btree ("projects_id");
  CREATE INDEX "payload_locked_documents_rels_cms_instances_id_idx" ON "webdock_admin"."payload_locked_documents_rels" USING btree ("cms_instances_id");
  CREATE INDEX "payload_locked_documents_rels_audit_events_id_idx" ON "webdock_admin"."payload_locked_documents_rels" USING btree ("audit_events_id");
  CREATE INDEX "payload_preferences_key_idx" ON "webdock_admin"."payload_preferences" USING btree ("key");
  CREATE INDEX "payload_preferences_updated_at_idx" ON "webdock_admin"."payload_preferences" USING btree ("updated_at");
  CREATE INDEX "payload_preferences_created_at_idx" ON "webdock_admin"."payload_preferences" USING btree ("created_at");
  CREATE INDEX "payload_preferences_rels_order_idx" ON "webdock_admin"."payload_preferences_rels" USING btree ("order");
  CREATE INDEX "payload_preferences_rels_parent_idx" ON "webdock_admin"."payload_preferences_rels" USING btree ("parent_id");
  CREATE INDEX "payload_preferences_rels_path_idx" ON "webdock_admin"."payload_preferences_rels" USING btree ("path");
  CREATE INDEX "payload_preferences_rels_users_id_idx" ON "webdock_admin"."payload_preferences_rels" USING btree ("users_id");
  CREATE INDEX "payload_migrations_updated_at_idx" ON "webdock_admin"."payload_migrations" USING btree ("updated_at");
  CREATE INDEX "payload_migrations_created_at_idx" ON "webdock_admin"."payload_migrations" USING btree ("created_at");`);
}

export async function down({ db }: MigrateDownArgs): Promise<void> {
  await db.execute(sql`
   DROP TABLE "webdock_admin"."users_sessions" CASCADE;
  DROP TABLE "webdock_admin"."users" CASCADE;
  DROP TABLE "webdock_admin"."customers" CASCADE;
  DROP TABLE "webdock_admin"."projects" CASCADE;
  DROP TABLE "webdock_admin"."cms_instances" CASCADE;
  DROP TABLE "webdock_admin"."audit_events" CASCADE;
  DROP TABLE "webdock_admin"."payload_kv" CASCADE;
  DROP TABLE "webdock_admin"."payload_locked_documents" CASCADE;
  DROP TABLE "webdock_admin"."payload_locked_documents_rels" CASCADE;
  DROP TABLE "webdock_admin"."payload_preferences" CASCADE;
  DROP TABLE "webdock_admin"."payload_preferences_rels" CASCADE;
  DROP TABLE "webdock_admin"."payload_migrations" CASCADE;
  DROP TYPE "webdock_admin"."enum_users_role";
  DROP TYPE "webdock_admin"."enum_customers_status";
  DROP TYPE "webdock_admin"."enum_projects_status";
  DROP TYPE "webdock_admin"."enum_cms_instances_provider";
  DROP TYPE "webdock_admin"."enum_cms_instances_template";
  DROP TYPE "webdock_admin"."enum_cms_instances_status";
  DROP TYPE "webdock_admin"."enum_audit_events_action";`);
}
