import Application from "../application";
export default function Page({params}: {params:Promise<{appID:string}>}) {return <Application params={params} view="logs"/>;}
