import 'server-only';
import {studioCall} from './studio-client';
import type {CanvasList,CanvasPage} from '@webdock/page-builder/model';
export const canvasList=(id:string)=>studioCall<CanvasList>('cms.canvasList',[id]);
export const canvasPage=(id:string,page:number)=>studioCall<CanvasPage>('cms.canvasPage',[id,page]);
export const writeCanvas=(id:string,input:unknown)=>studioCall<{page?:CanvasPage;homePageID?:number|null}>('cms.writeCanvas',[id,input]);

export type {CanvasCommand} from '@webdock/api-client/cms';
