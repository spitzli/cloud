import 'server-only';
import {HostingError} from '@webdock/hosting-contracts';
import type {DatabaseCommand} from '@webdock/database-contracts';
import {studioCall} from './studio-client';
import {AccessError,StudioError} from './studio-errors';
import {HostingSessionRequiredError} from './hosting-page-access';
export async function databaseCall<T>(command:DatabaseCommand):Promise<T>{
 try{return await studioCall<T>('database.command',[command]);}
 catch(error){if(error instanceof AccessError&&error.status===401)throw new HostingSessionRequiredError();if(error instanceof StudioError)throw new HostingError(error.status,error.message);throw error;}
}
