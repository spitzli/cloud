export type WebsiteContentField = {key:string;variant:'modern'|'old';kind:'text'|'image';label:string;value:string;updatedAt:string;section?:string};

export function parseContentField(value:unknown): WebsiteContentField {
  const r=value as WebsiteContentField;
  if(!r || !['modern','old'].includes(r.variant) || !['text','image'].includes(r.kind) || typeof r.key!=='string' || !/^(modern|old)_(text|image)_[0-9]{3}$/.test(r.key) || typeof r.label!=='string' || r.label.length>160 || typeof r.value!=='string' || r.value.length>6000 || typeof r.updatedAt!=='string' || (r.section!==undefined && (typeof r.section!=='string' || !r.section.trim() || r.section.length>160))) throw Error('Ungültige Antwort der Inhaltsverwaltung.');
  return {key:r.key,variant:r.variant,kind:r.kind,label:r.label,value:r.value,updatedAt:r.updatedAt,...(r.section!==undefined?{section:r.section}:{})};
}

export function contentSections(fields:WebsiteContentField[]) {
  // Legacy numeric field keys contain no reliable page/section information.
  return [...new Set(fields.flatMap(field=>field.section?[field.section]:[]))];
}
