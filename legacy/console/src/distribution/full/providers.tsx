"use client";
import { DatabaseLaunchProvider } from "@/components/databases/action-boundary";
export function ProductProvider({children}:{children: React.ReactNode}) { return <DatabaseLaunchProvider adapter={false}>{children}</DatabaseLaunchProvider>; }
