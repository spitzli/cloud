import Deployments from "../deployments";
export default function Page(props: {params:Promise<{projectID:string}>;searchParams:Promise<{connection?:string;page?:string;releasePage?:string}>}) { return <Deployments {...props} view="setup"/>; }
