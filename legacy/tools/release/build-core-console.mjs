import { cp, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { resolve, relative, dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
const root = resolve(fileURLToPath(new URL('../..', import.meta.url)));
export function coreRuntimeProductPath(path) {
  return /(?:\/(?:modules\/(?:mail|hosting|databases|storage|cms|billing)|adapters\/(?:hosting|mail|database|storage))\/|\/packages\/(?:hosting-|mail-|database-contracts|payload-|page-builder)|\/@webdock\/(?:module-(?!sdk)|adapter-|hosting-|mail-|database-contracts|payload-|page-builder))/.test(path);
}
export function coreConsolePath(path) {
  if (path.split('/').some(part => part.startsWith('@'))) return true;
  const parts = path.replaceAll('\\','/').split('/').filter(part => part && !part.startsWith('(') && !part.startsWith('@'));
  const directories = parts.slice(0,-1);
  if (!directories.length) return true;
  const [area, , section] = directories;
  if (['projects','customers'].includes(area)) return directories.length <= 2;
  if (area === 'tenants') return directories.length <= 2 || (directories.length === 3 && ['profile','team'].includes(section));
  if (area === 'admin') return directories.length === 1 || directories[1] === 'registry-users';
  return ['activity','plugins','people','account','login','sign-in','sign-up','access-pending','waitlist'].includes(area) || directories.join('/') === 'console/search' || directories.join('/') === 'api/locale' || directories.join('/') === 'auth/integration-resume';
}
async function files(directory) {
  const result = [];
  for (const entry of await readdir(directory,{withFileTypes:true})) {
    const path=join(directory,entry.name);
    if(entry.isDirectory()) result.push(...await files(path)); else result.push(path);
  }
  return result;
}
export async function assembleCoreConsole(destination) {
  const source=join(root,'apps/console');
  await mkdir(destination,{recursive:true});
  await cp(join(source,'src'),join(destination,'src'),{recursive:true});
  const routes=[];
  for(const file of await files(join(destination,'src/app'))) {
    const name=relative(join(destination,'src/app'),file);
    if(!coreConsolePath(name)) await rm(file); else if(/(?:page|route)\.tsx?$/.test(name)) routes.push(name);
  }
  for(const file of await files(join(destination,'src/distribution/full'))) await rm(file);
  await cp(join(source,'public'),join(destination,'public'),{recursive:true});
  await rm(join(destination,'public/hosting-agent.zip'),{force:true});
  const config=JSON.parse(await readFile(join(source,'tsconfig.json'),'utf8'));
  config.compilerOptions.paths['@distribution/*']=['./src/distribution/core/*'];
  config.include=['next-env.d.ts','src/app/**/*.ts','src/app/**/*.tsx','src/proxy.ts','.next/types/**/*.ts'];
  await writeFile(join(destination,'tsconfig.json'),JSON.stringify(config,null,2)+'\n');
  const manifest=JSON.parse(await readFile(join(source,'package.json'),'utf8'));
  manifest.name='@webdock/core-admin';
  for(const name of Object.keys(manifest.dependencies)) if(/^@webdock\/(?:module-(?!sdk)|hosting-|database-contracts|mail-|payload-|page-builder)/.test(name)) delete manifest.dependencies[name];
  await writeFile(join(destination,'package.json'),JSON.stringify(manifest,null,2)+'\n');
  await writeFile(join(destination,'next.config.ts'),`import base from ${JSON.stringify(join(source,'next.config.ts'))};
const forbidden=/^@webdock\\/(?:module-(?:mail|hosting|databases|storage|cms)(?:\\/|$)|adapter-|hosting-|database-contracts(?:\\/|$)|mail-(?:contracts|core)(?:\\/|$)|payload-|page-builder(?:\\/|$))/;
export default {...base,transpilePackages:base.transpilePackages?.filter(name=>!forbidden.test(name)),webpack(config){config.plugins.push({apply(compiler){compiler.hooks.normalModuleFactory.tap('core-no-products',factory=>factory.hooks.beforeResolve.tap('core-no-products',request=>{if(request && forbidden.test(request.request))throw Error('Forbidden core Console product import '+request.request);}));}});return config;}};
`);
  routes.sort();
  await writeFile(join(destination,'core-routes.json'),JSON.stringify(routes,null,2)+'\n');
  return routes;
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const target=resolve(process.argv[2] || join(root,'.core-build/console'));
  if(!target.startsWith(join(root,'.core-build')+'/')) throw Error('Core Console build directory must be an owned repository subdirectory');
  await rm(target,{recursive:true,force:true});
  const routes=await assembleCoreConsole(target);
  const result=spawnSync(process.execPath,[join(root,'node_modules/next/dist/bin/next'),'build','--webpack'],{cwd:target,stdio:'inherit',env:{...process.env,NEXT_TELEMETRY_DISABLED:'1'}});
  if(result.status!==0)process.exit(result.status||1);
  const standalone=join(target,'.next/standalone');
  const server=(await files(standalone)).find(file=>file.endsWith('/server.js') && !file.includes('/node_modules/'));
  if(!server)throw Error('Standalone Console server missing');
  await cp(join(target,'.next/static'),join(dirname(server),'.next/static'),{recursive:true});
  await cp(join(target,'public'),join(dirname(server),'public'),{recursive:true});
  console.log(JSON.stringify({server,routes:routes.length}));
}
