import {test} from 'node:test';
import assert from 'node:assert/strict';
import {buildDatabaseRls} from '../src/db/database-rls-schema';
test('reviewed database routes are operator-managed and readable only by their source agent',()=>{
 const {sql,unknownTables}=buildDatabaseRls([{schema:'webdock_auth',name:'hosting_database_binding_route',columns:['id','binding_id','customer_id','project_id','source_cluster_id']}]);
 assert.deepEqual(unknownTables,[]);
 assert.match(sql,/webdock_auth_runtime_read[\s\S]*USING\(webdock_rls.operator\(\)\)/);
 assert.match(sql,/webdock_worker_runtime_read[\s\S]*'agent'[\s\S]*"hosting_database_binding_route"\."source_cluster_id"/);
 assert.match(sql,/webdock_worker_runtime_insert[^;]+WITH CHECK\(false\)/);
 assert.match(sql,/webdock_worker_runtime_update[^;]+WITH CHECK\(false\)/);
 assert.match(sql,/webdock_worker_runtime_delete[^;]+USING\(false\)/);
 assert.match(sql,/REVOKE INSERT,UPDATE,DELETE ON "webdock_auth"\."hosting_database_binding_route" FROM webdock_worker_runtime/);
 assert.match(sql,/REVOKE DELETE ON "webdock_auth"\."hosting_database_binding_route" FROM webdock_auth_runtime/);
 assert.doesNotMatch(sql,/GRANT .* ON "webdock_auth"\."hosting_database_binding_route" TO webdock_auth_bootstrap/);
});
