import {redirect} from 'next/navigation';
export default async function Page({params}:{params:Promise<{customerID:string}>}){const {customerID}=await params;redirect(`/tenants/${customerID}/team`);}
