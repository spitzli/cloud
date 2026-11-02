// Run after disconnecting old automatic Neon project links. No credential values are logged.
import fs from 'node:fs';
import path from 'node:path';
import {parseEnv} from 'node:util';
const dir=path.resolve(process.argv[2]||'');
if(!process.argv[2])throw new Error('Usage: bun scripts/sync-instance-env.mjs /path/to/instance');
const project=JSON.parse(fs.readFileSync(path.join(dir,'.vercel/project.json'),'utf8'));
const env=parseEnv(fs.readFileSync(path.join(dir,'.env.instance'),'utf8'));
const auth=JSON.parse(fs.readFileSync(path.join(process.env.HOME,'.local/share/com.vercel.cli/auth.json'),'utf8'));
const keys=['DATABASE_URL','DATABASE_URL_UNPOOLED','PAYLOAD_SECRET','OPERATOR_EMAIL','NEXT_PUBLIC_SERVER_URL','SITE_URL','BLOB_READ_WRITE_TOKEN','SMTP_HOST','SMTP_PORT','SMTP_FROM','SMTP_USER','SMTP_PASS','SMTP_PASSWORD','ADMIN_EMAIL','CONTACT_ENABLED','HCAPTCHA_SITE_KEY','HCAPTCHA_SECRET','MAINTENANCE_BYPASS_KEY','PREVIEW_SECRET','CRON_SECRET'];
for(const key of ['DATABASE_URL','DATABASE_URL_UNPOOLED','PAYLOAD_SECRET','OPERATOR_EMAIL'])if(!env[key])throw new Error(`Missing ${key}`);
const base=`https://api.vercel.com/v10/projects/${project.projectId}/env?teamId=${project.orgId}`;
const headers={Authorization:`Bearer ${auth.token}`,'Content-Type':'application/json'};
const before=await(await fetch(base,{headers})).json();
if(before.envs?.some(e=>e.key==='DATABASE_URL'&&e.contentHint?.type==='integration-store-secret'))throw new Error('Disconnect the old Neon resource first');
const values=keys.filter(key=>env[key]!==undefined).map(key=>({key,value:env[key],type:'encrypted',target:['production','preview','development']}));
const response=await fetch(base+'&upsert=true',{method:'POST',headers,body:JSON.stringify(values)});
if(!response.ok)throw new Error(`Vercel env update failed (${response.status})`);
const result=await response.json();if(result.error||result.failed?.length)throw new Error('Vercel reported failed environment updates');
console.log(`${path.basename(dir)}: ${values.length} scoped environment values synchronized.`);
// Local development must use the same isolated instance, not a stale central connection.
const local=path.join(dir,'.env.local');
if(fs.existsSync(local)&&!fs.existsSync(local+'.before-instance'))fs.copyFileSync(local,local+'.before-instance');
const previous=fs.existsSync(local)?parseEnv(fs.readFileSync(local,'utf8')):{};
for(const key of Object.keys(previous))if(/^(CMS_|FORM_API_KEY$|PG|POSTGRES_|NEON_PROJECT_ID$)/.test(key))delete previous[key];
fs.writeFileSync(local,Object.entries({...previous,...env}).map(([k,v])=>`${k}=${JSON.stringify(v)}`).join('\n')+'\n',{mode:0o600});
