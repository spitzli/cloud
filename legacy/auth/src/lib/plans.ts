import "server-only";
import { studioCall } from "./studio-client";
export { PlanError } from "./studio-errors";
import type { ChangeResult, Plan, TenantPlanView, OfferView } from "./studio-contracts";
export function getPlans(...args: [headers: Headers]): Promise<Plan[]> { return studioCall("getPlans", args.slice(1)); }
export function getTenantPlan(...args: [headers: Headers, customerID: string]): Promise<TenantPlanView> { return studioCall("getTenantPlan", args.slice(1)); }
export function managePlans(...args: [headers: Headers, input: Record<string, string>]): Promise<ChangeResult & { offerURL?: string }> { return studioCall("managePlans", args.slice(1)); }
export function getOffer(...args: [headers: Headers, token: string]): Promise<OfferView> { return studioCall("getOffer", args.slice(1)); }
export function acceptOffer(...args: [headers: Headers, token: string]): Promise<ChangeResult & { customerID: string }> { return studioCall("acceptOffer", args.slice(1)); }
export type { Allowances, Plan } from "./studio-contracts";
