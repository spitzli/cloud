import path from "node:path";
import type { NextConfig } from "next";
import { withPayload } from "@payloadcms/next/withPayload";
const config: NextConfig = {
  output: "standalone",
  outputFileTracingRoot: path.resolve(process.cwd(), "../.."),
  transpilePackages: [
    "@spitzli/cms-ui",
    "@spitzli/cms-bridge",
    "@spitzli/payload-sso",
    "@spitzli/instance-kit",
    "@spitzli/i18n",
  ],
  turbopack: { root: path.resolve(process.cwd(), "../..") },
};
export default withPayload(config);
