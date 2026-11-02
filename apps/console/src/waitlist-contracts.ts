import {z} from 'zod';
const contact=z.string().trim().toLowerCase().max(254).pipe(z.email());
export const waitlistSubmission=z.object({email:contact,name:z.string().trim().max(100).refine(value=>Array.from(value).every(char=>char.charCodeAt(0)>=32&&(char.charCodeAt(0)<127||char.charCodeAt(0)>159))).optional(),privacyAccepted:z.literal(true)}).strict();
export const waitlistDecision=z.object({action:z.enum(['approve','reject']),version:z.number().int().min(1).max(2147483646)}).strict();
export const waitlistEntry=z.object({id:z.string().regex(/^[1-9][0-9]{0,18}$/),email:z.string().min(1).max(254),name:z.string().nullable(),status:z.enum(['pending','approved','rejected']),createdAt:z.iso.datetime({offset:true}),reviewedAt:z.iso.datetime({offset:true}).nullable(),version:z.number().int().positive()}).strict();
export const waitlistPage=z.object({entries:z.array(waitlistEntry).max(100),nextCursor:z.string().nullable()}).strict();
export const waitlistAccepted=z.object({accepted:z.literal(true)}).strict();
export type WaitlistEntry=z.infer<typeof waitlistEntry>;
