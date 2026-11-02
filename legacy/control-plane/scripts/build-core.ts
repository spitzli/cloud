import { mkdir, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
const root=fileURLToPath(new URL('../',import.meta.url));
/** Generic PostgreSQL transport remains core infrastructure; product packages may not enter this artifact. */
export const forbiddenCoreImport=/^(?:@webdock\/(?:module-(?:mail|hosting|databases|storage|cms)(?:\/|$)|adapter-(?!database-postgres(?:\/|$))|mail-(?:core|contracts)(?:\/|$)|hosting-|database-contracts(?:\/|$)|cms(?:-|\/|$)|payload-|page-builder(?:\/|$))|payload(?:\/|$)|@payloadcms\/)/;
export async function buildCoreArtifact(destination=root+'dist-core'){
 await mkdir(destination,{recursive:true});
 const result=await Bun.build({entrypoints:[root+'src/core.ts'],target:'bun',outdir:destination,plugins:[{name:'deny-product-imports',setup(build){build.onResolve({filter:forbiddenCoreImport},({path,importer})=>{throw Error(`Core cannot import ${path} from ${importer}`);});}}]});
 if(!result.success)throw new AggregateError(result.logs,'Core build failed');
 const files=[];
 for(const file of result.outputs){const bytes=Buffer.from(await file.arrayBuffer());files.push({file:file.path.slice(destination.length+1),size:bytes.length,sha256:createHash('sha256').update(bytes).digest('hex')});}
 await writeFile(destination+'/manifest.json',JSON.stringify({distribution:'core',runtime:'bun',products:[],files},null,2)+'\n');
 return files;
}
if(import.meta.main)console.log(JSON.stringify(await buildCoreArtifact(),null,2));
