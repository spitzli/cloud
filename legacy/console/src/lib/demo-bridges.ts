import 'server-only';
import {studioCall} from './studio-client';
import type {DemoAccess,DemoBridge,DemoRequest,CalendarState} from '@webdock/api-client/cms';
export type {DemoRequest,CalendarWindow,CalendarSettings,DemoAppointment,CalendarState} from '@webdock/api-client/cms';
export const demoBridges=()=>studioCall<Record<string,DemoBridge>>('cms.bridges');
export const demoAccess=(id:string,write=false)=>studioCall<DemoAccess>('cms.access',[id,write]);
export const demoRequest=(id:string,page=1,body?:Record<string,unknown>)=>studioCall<{docs:DemoRequest[];page:number;hasNextPage:boolean}>('cms.requests',[id,page,body??null]);
export const demoCalendarRequest=(id:string,body?:Record<string,unknown>,page=1)=>studioCall<CalendarState>('cms.calendar',[id,body??null,page]);
