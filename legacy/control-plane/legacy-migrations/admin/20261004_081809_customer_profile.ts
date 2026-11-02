import { MigrateUpArgs, MigrateDownArgs, sql } from '@payloadcms/db-postgres'

export async function up({ db }: MigrateUpArgs): Promise<void> {
  await db.execute(sql`
   CREATE TYPE "webdock_admin"."enum_customers_customer_type" AS ENUM('person', 'company');
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "customer_type" "webdock_admin"."enum_customers_customer_type" DEFAULT 'company';
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "first_name" varchar;
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "last_name" varchar;
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "company_name" varchar;
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "phone" varchar;
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "address_line1" varchar;
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "address_line2" varchar;
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "postal_code" varchar;
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "city" varchar;
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "region" varchar;
  ALTER TABLE "webdock_admin"."customers" ADD COLUMN "country" varchar;`)
}

export async function down({ db }: MigrateDownArgs): Promise<void> {
  await db.execute(sql`
   ALTER TABLE "webdock_admin"."customers" DROP COLUMN "customer_type";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "first_name";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "last_name";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "company_name";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "phone";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "address_line1";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "address_line2";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "postal_code";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "city";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "region";
  ALTER TABLE "webdock_admin"."customers" DROP COLUMN "country";
  DROP TYPE "webdock_admin"."enum_customers_customer_type";`)
}
