import 'server-only';
import {studioCall} from './studio-client';
import type {Customer,Project,CmsInstance} from '../payload-types';
export const writeCustomer=(_actor:unknown,id:string|null,input:unknown)=>studioCall<Customer>('registry.customer',[id,input]);
export const writeProject=(_actor:unknown,id:string|null,input:unknown)=>studioCall<Project>('registry.project',[id,input]);
export const writeInstance=(_actor:unknown,id:string|null,input:unknown,confirmExisting:boolean)=>studioCall<CmsInstance>('registry.instance',[id,input,confirmExisting]);
export const setArchived=(_actor:unknown,collection:'customers'|'projects',id:string,archived:boolean)=>studioCall<Customer|Project>('registry.archive',[collection,id,archived]);
