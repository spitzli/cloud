# Free canvas and live preview Implementation Plan

> **For agentic workers:** Use superpowers:subagent-driven-development or superpowers:executing-plans. Existing uncommitted user work must be preserved. No extra UI framework is needed.

**Goal:** Author and publish freely positioned responsive pages, with unsaved live preview inside Webdock.
**Architecture:** A shared model/validator/HTML renderer is imported by Studio and vendored into each standalone Payload runtime. Payload stores draft/published snapshots with revision checks. New pages publish at `/p/{slug}`; an explicit reversible action can use a published page as the homepage. Original lead `/old` and Pizza ordering remain available.
**Tech Stack:** Node24, Next16, React19, TypeScript, native Pointer Events, existing PostgreSQL/Payload.
**Spec:** `docs/superpowers/specs/2026-10-05-customer-view-and-canvas.md` and the exact contract below.

## Global constraints
- Free canvas, not predefined sections. Text, image, box and button primitives are freely positioned.
- One deterministic renderer for current local state and published state. Preview is not a screenshot or a fetch of the old live page.
- No user JavaScript, HTML, CSS strings or credentials in the canvas.
- Existing websites stay active until explicit homepage activation; activation can be reverted.
- Native APIs remain private; Studio performs current binding/role/preview-mode checks on every operation.
- No deployment, env replacement or migration by subagents without Root coordination.

## Review focus
- Malformed documents, duplicate IDs, unsafe URL schemes and CSS injection must fail validation.
- Save/publish races return409; public output must never read drafts.
- Pointer zoom conversion, resize handles, keyboard inputs and undo must not corrupt geometry.
- Client draft data must never be silently replaced by remote revalidation or published by viewing a preview.
- Mobile geometry, readonly tenant-preview mode and legacy booking/storefront routes remain usable.

## Exact shared model contract

Package: `packages/page-builder`, exports `@spitzli/page-builder/model` and `@spitzli/page-builder/render`. Source is pure TypeScript with no new runtime dependency.

```ts
export type Breakpoint = 'desktop' | 'tablet' | 'mobile';
export type NodeType = 'text' | 'image' | 'box' | 'button';
export type Frame = { x:number; y:number; width:number; height:number; hidden:boolean };
export type NodeStyle = {
  background:string; color:string; fontFamily:'sans'|'serif'|'mono';
  fontSize:number; fontWeight:number; align:'left'|'center'|'right'; lineHeight:number;
  borderColor:string; borderWidth:number; radius:number; opacity:number; fit:'cover'|'contain';
};
export type CanvasNode = {
  id:string; name:string; type:NodeType;
  text:string; src:string; alt:string; href:string;
  frames:Record<Breakpoint,Frame>; styles:Record<Breakpoint,NodeStyle>;
};
export type CanvasDocument = {
  schemaVersion:1; title:string; slug:string;
  artboards:Record<Breakpoint,{width:number;height:number;background:string}>;
  nodes:CanvasNode[];
};
export type CanvasPage = {
  id:number; draft:CanvasDocument; draftRevision:number;
  published:CanvasDocument|null; publishedRevision:number|null;
  publishedAt:string|null; updatedAt:string;
};
export type CanvasPageSummary = {
  id:number; title:string; slug:string; draftRevision:number;
  publishedRevision:number|null; publishedSlug:string|null; updatedAt:string;
};
export type CanvasList = {pages:CanvasPageSummary[];homePageID:number|null;nativePath:string};
export function parseCanvasDocument(value:unknown):CanvasDocument;
export function createCanvasDocument(title?:string,slug?:string):CanvasDocument;
export function createCanvasNode(type:NodeType):CanvasNode;
export function safeCanvasURL(value:string,kind:'link'|'image',origin:string):string|null;
export function renderCanvasHTML(document:CanvasDocument,options:{origin:string;breakpoint?:Breakpoint;interactive?:boolean;favicon?:string}):string;
export function renderCanvasFragment(document:CanvasDocument,options:{origin:string;breakpoint?:Breakpoint;interactive?:boolean}):string;
```

Bounds: schema1 only; exact known keys; document title1..120, slug lowercase letters/digits/hyphens1..64; <=100 nodes; <=512KiB UTF8 JSON. IDs nonempty unique `[A-Za-z0-9_-]{1,80}`. Text<=8000, name/alt<=200, href/src<=2000. Colors only hex3/6/8 or `transparent`. All numbers finite. Artboard width desktop1024..2560, tablet768..1023, mobile320..767; heights100..20000. Defaults1440/768/390 and1000 height. Node x/y within +/-20000; width/height1..20000; fontSize8..240, weight100..900 integer, lineHeight0.8..3, border0..24, radius0..1000, opacity0..1. URL links allow HTTPS, site-local `/...` (not `//` or backslash), fragment, mailto, tel; images only HTTPS/site-local. Never data/javascript URLs or URL credentials. Empty image/button URL is allowed while drafting and renders as placeholder/inert control. The renderer must escape text and attributes even when invoked with transient UI documents.

Responsive rule: public output chooses mobile below768, tablet768..1023, desktop>=1024. Coordinates, boxes and font sizes scale proportionally within the selected artboard width. Preview/editor may force a named breakpoint. The same geometry/styles must render in all contexts. Parent artboard clips overflowing nodes. Node array order defines layer and DOM reading order. Plain text preserves newlines. Preview has inert links and no script execution.

## Private site bridge contract

Endpoint `/api/webdock/canvas`, headers existing Bearer bridge secret, `X-Webdock-Role` (reader/editor/admin/operator), `X-Webdock-Actor` (actual authenticated ID).

GET without id returns `CanvasList`; GET `?id=N` returns `{page:CanvasPage}`.
POST discriminated commands:
```ts
{action:'create',document:CanvasDocument}
{action:'save',id:number,expectedRevision:number,document:CanvasDocument}
{action:'publish',id:number,expectedRevision:number}
{action:'unpublish',id:number,expectedRevision:number}
{action:'homepage',id:number|null,expectedRevision?:number}
```
create/save/publish/unpublish return `{page:CanvasPage}`. homepage returns `{homePageID:number|null}`. Homepage activation requires admin/operator and a currently published exact expected draftRevision; reject unpublished/unsaved mismatch. Unpublish active homepage reverts to original atomically. All writes are transactions with row locks. Readers GET only. Existing bridge secret format, exact-origin configuration and bounded response handling stay in force. No GET creates data or provider credentials.

Native collection `site-pages`: draft JSON, revision integer, published JSON nullable, publishedRevision nullable, publishedSlug unique nullable, publishedAt nullable, updatedBy/publishedBy server strings plus createdAt/updatedAt. Draft title/slug can be projected from JSON. Global `site-builder`: JSON settings `{homePageID:number|null}`, initialized by migration, no public native access. Preserve original published URL while draft slug changes. Record publisher and revision without request secrets. Limit list to100 page summaries; reject creation above100 pages rather than silently hiding records.

Public route `/p/[slug]` reads only published snapshot. Lead root route uses activated published canvas or original html('modern'); `/old` unchanged. Pizza adds `/shop` for the original Storefront and uses activated published canvas at `/` only when explicitly selected. Original mode is default and reversible. All demo public pages retain noindex and their existing favicon.

## Tasks

### 1. Shared model and renderer
Files: new `packages/page-builder/package.json`, `src/model.ts`, `src/render.ts`, tests.
- [x] Failing validation/escaping tests for bounds, duplicate IDs, malicious strings, geometry, links and current unsaved values.
- [x] Implement types/defaults/validation and deterministic renderer with editor fragment and sandboxed full preview document output.
- [x] Prove forced-preview and public breakpoint output use identical node values/styles.

### 2. Site persistence and runtime integration
Files: new shared/vendor canvas store/collection/routes, template and Pizza configs/migrations/public routes; explicit sync script.
- [x] Test real Postgres save conflicts, published isolation, slug changes, homepage activation/revert, role denial and independent schema isolation.
- [x] Implement bounded private bridge and Payload collection/global with native access denied.
- [x] Add additive migrations; preserve existing source HTML and storefront behavior.
- [x] Add vendor sync using the existing workspace pattern. Subagent does not deploy or migrate production.

### 3. Studio editor
Files: new components/canvas/* plus sites/[bindingID]/builder/page.tsx and actions.ts; Root owns Studio canvas-bridge and module activation.
- [x] Layers, free stage, numeric/style inspector, add/select/drag/resize/reorder/duplicate/delete, keyboard movement and undo/redo.
- [x] Separate create/select/save/publish/unpublish/homepage controls; dirty indicator, version conflicts and navigation protection.
- [x] Render live unsaved preview through shared renderer in isolated srcDoc iframe; no private fetch or script capability in iframe.
- [x] Readonly customer-preview mode retains viewing/preview, blocks every mutation in UI and server.

### 4. Rollout and end-to-end verification
- [x] Enable Canvas capability only after each site backend passes migration/API checks.
- [x] Deploy a pilot lead and Pizza, test create→save→unsaved preview→publish→public page→unpublish with named QA records and cleanup only those records.
- [x] Roll out remaining14 sites and activate capabilities safely using round-trip-tested env serializer.
- [x] Verify desktop/320/375/414/768, pointer+keyboard, real session roles and existing root/old/native shop behavior.
