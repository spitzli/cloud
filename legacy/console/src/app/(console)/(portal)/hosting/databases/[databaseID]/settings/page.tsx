import DatabaseWorkspace from "../database-workspace";
export default async function Page({
  params,
}: {
  params: Promise<{ databaseID: string }>;
}) {
  return <DatabaseWorkspace params={params} view="settings" />;
}
