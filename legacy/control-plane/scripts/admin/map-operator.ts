import { offlineOwner } from "../offline-owner";
import { recordID } from "../../src/modules/registry/contracts";
const subject = recordID.parse(process.argv[4]),
  sessionID = recordID.parse(process.argv[5]);
const { pool } = await offlineOwner(process.argv[2], process.argv[3]);
try {
  await pool.query("BEGIN");
  const user = (
    await pool.query(
      `SELECT u.id,u.name,u.email FROM webdock_auth."user" u JOIN webdock_auth.session s ON s."userId"=u.id WHERE u.id=$1 AND s.id=$2 AND s."expiresAt">now() AND u.role='operator' AND u."twoFactorEnabled" AND u."emailVerified" AND NOT u."mustChangePassword" AND NOT coalesce(u.banned,false) FOR SHARE OF u,s`,
      [subject, sessionID],
    )
  ).rows[0];
  if (!user)
    throw Error(
      "A current operator session with completed security setup is required",
    );
  const existing = (
    await pool.query(
      "SELECT id,role,auth_subject FROM webdock_admin.users WHERE auth_subject=$1 OR lower(email)=lower($2) FOR UPDATE",
      [subject, user.email],
    )
  ).rows;
  if (
    existing.length > 1 ||
    (existing[0] &&
      (existing[0].role !== "operator" ||
        (existing[0].auth_subject && existing[0].auth_subject !== subject)))
  )
    throw Error("Existing native identity mapping differs");
  let id = existing[0]?.id;
  if (id)
    await pool.query(
      "UPDATE webdock_admin.users SET auth_subject=$1 WHERE id=$2",
      [subject, id],
    );
  else
    id = (
      await pool.query(
        "INSERT INTO webdock_admin.users(id,name,email,role,auth_subject) VALUES(webdock_admin.next_snowflake(),$1,$2,'operator',$3) RETURNING id",
        [user.name, user.email, subject],
      )
    ).rows[0].id;
  await pool.query("COMMIT");
  console.log(JSON.stringify({ mappedOperatorID: id }));
} catch (error) {
  await pool.query("ROLLBACK");
  throw error;
} finally {
  await pool.end();
}
