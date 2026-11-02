import {createRequire} from 'node:module';import {dirname,join} from 'node:path';import {mkdir,copyFile,readFile} from 'node:fs/promises';
const require=createRequire(import.meta.url);
const root=dirname(dirname(require.resolve('@scalar/api-reference')));
const {version}=JSON.parse(await readFile(join(root,'package.json'),'utf8'));
const output=new URL(`../public/vendor/scalar/${version}/`,import.meta.url);
await mkdir(output,{recursive:true});await copyFile(join(root,'dist/browser/standalone.js'),new URL('standalone.js',output));
