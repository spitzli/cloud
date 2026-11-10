import type { NextConfig } from "next";
import { withPayload } from "@payloadcms/next/withPayload";
const config: NextConfig = {
  output: "standalone",
  transpilePackages: [
    "@spitzli/cms-ui",
    "@spitzli/cms-bridge",
    "@spitzli/payload-sso",
    "@spitzli/instance-kit",
    "@spitzli/i18n",
  ],
};
export default withPayload(config);
