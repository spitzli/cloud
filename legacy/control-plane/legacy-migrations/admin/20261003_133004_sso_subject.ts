import { MigrateUpArgs, MigrateDownArgs, sql } from '@payloadcms/db-postgres'

export async function up({ db }: MigrateUpArgs): Promise<void> {
  await db.execute(sql`
   ALTER TABLE "webdock_admin"."users" ADD COLUMN "auth_subject" varchar;
  CREATE UNIQUE INDEX "users_auth_subject_idx" ON "webdock_admin"."users" USING btree ("auth_subject");`)
}

export async function down({ db }: MigrateDownArgs): Promise<void> {
  await db.execute(sql`
   DROP INDEX "webdock_admin"."users_auth_subject_idx";
  ALTER TABLE "webdock_admin"."users" DROP COLUMN "auth_subject";`)
}
