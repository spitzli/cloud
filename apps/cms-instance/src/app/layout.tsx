import type { ReactNode } from "react";
import "@spitzli/cms-ui/styles.css";
export default function Layout({ children }: { children: ReactNode }) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
