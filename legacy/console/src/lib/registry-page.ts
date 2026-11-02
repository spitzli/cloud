import 'server-only';
import {cache} from 'react';
import {redirect} from 'next/navigation';
import {studioCall} from './studio-client';
import {registryClient} from './registry-client';
import {StudioError,AccessError} from './studio-errors';
import type {User} from '../payload-types';
export const requireRegistryOperator=cache(async()=>{
 try{
  const user=await studioCall<User>('registry.session');
  if(user?.collection!=='users'||user.role!=='operator')redirect('/tenants');
  return{payload:registryClient,user};
 }catch(error){
  if(error instanceof AccessError&&error.status===401)redirect('/api/sso/login');
  if(error instanceof StudioError&&error.status===403)redirect('/tenants');
  throw error;
 }
});
