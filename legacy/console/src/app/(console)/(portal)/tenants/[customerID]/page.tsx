import { TenantWorkspace } from "./workspace";
export { generateMetadata } from "./workspace";
export default function Page({params}: {params: Promise<{customerID: string}>}) { return <TenantWorkspace params={params}/>; }
