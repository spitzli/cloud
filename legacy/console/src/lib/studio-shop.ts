import 'server-only';
import {studioCall} from './studio-client';
import type {ShopKind,ShopResult,DemoAccess} from '@webdock/api-client/cms';
export type {ShopKind,ShopOption,ShopProduct,ShopContent,ShopOrder,ShopAddress,ShopMedia,ShopCustomer,ShopResult} from '@webdock/api-client/cms';
export const shopAccess=(id:string,kind:ShopKind,write=false)=>studioCall<DemoAccess>('cms.shopAccess',[id,kind,write]);
export const shopRequest=(id:string,kind:ShopKind,page=1,body?:Record<string,unknown>)=>studioCall<ShopResult>('cms.shop',[id,kind,page,body??null]);
