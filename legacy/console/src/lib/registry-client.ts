import 'server-only';
import { cache } from 'react';
import {studioCall} from './studio-client';
import type {User,Customer,Project,CmsInstance,AuditEvent} from '../payload-types';
type Records={users:User;customers:Customer;projects:Project;'cms-instances':CmsInstance;'audit-events':AuditEvent};
type Collection=keyof Records;
type Query<C extends Collection>={collection:C;where?:unknown;id?:string;page?:number;limit?:number;sort?:string|string[];depth?:number;pagination?:boolean;select?:unknown;disableErrors?:boolean;user?:unknown;overrideAccess?:boolean};
export type FindResult<T>={docs:T[];totalDocs:number;totalPages:number;page:number;limit:number;hasNextPage:boolean;hasPrevPage:boolean;nextPage:number|null;prevPage:number|null};
function input<C extends Collection>(query:Query<C>){const{user,overrideAccess,...options}=query;void user;void overrideAccess;return options;}
// Primitive keys let separately constructed queries share only the current render's read.
const registryRead = cache(<T>(method: string, options: string): Promise<T> =>
 studioCall('registry.query', [method, JSON.parse(options)]),
);
export const registryClient={
 async find<C extends Collection>(query:Query<C>):Promise<FindResult<Records[C]>>{
  const options=input(query);
  if(query.pagination!==false||query.limit!==undefined)return registryRead('find',JSON.stringify(options));
  const first=await registryRead<FindResult<Records[C]>>('find',JSON.stringify({...options,limit:1000}));
  const docs=[...first.docs];let page=first.page;
  while(page<first.totalPages){page++;const next=await registryRead<FindResult<Records[C]>>('find',JSON.stringify({...options,page,limit:1000}));docs.push(...next.docs);}
  return{...first,docs,hasNextPage:false,nextPage:null};
 },
 findByID<C extends Collection>(query:Query<C>&{id:string}):Promise<Records[C]>{return registryRead('findByID',JSON.stringify(input(query)));},
 count<C extends Collection>(query:Query<C>):Promise<{totalDocs:number}>{return registryRead('count',JSON.stringify(input(query)));},
};
