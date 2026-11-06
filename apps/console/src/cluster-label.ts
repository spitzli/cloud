/** Preserve stored cluster identities while displaying the current platform brand. */
export function clusterLabel(name: string): string {
  return name.replace(/^Webdock Cluster\b/u, "Spitzli Cloud Cluster");
}
