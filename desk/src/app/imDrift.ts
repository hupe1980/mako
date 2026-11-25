// Whether a service is in drift — serving a typed tool with another schema
// than the Desk was generated against — as the banner (`Drift.tsx`) last
// found it. Read from the query cache only, so a view can withhold a
// decision without loading the fingerprints itself.

import { skipToken, useQuery } from "@tanstack/react-query";
import type { Service } from "../data/mcp";

/** The query the banner fills: the drifted tools of `service`, or null when it lists none. */
export const driftKey = (service: Service) => ["drift", service] as const;

/** The query the banner fills for a REST document: the served version when it differs. */
export const driftRestKey = (service: string) => ["drift-rest", service] as const;

/** True while the banner's last check found `service` in drift, on either door. */
export function useImDrift(service: Service): boolean {
  const mcp = useQuery<string[] | null>({ queryKey: driftKey(service), queryFn: skipToken });
  const rest = useQuery<string[] | null>({ queryKey: driftRestKey(service), queryFn: skipToken });
  return (mcp.data?.length ?? 0) > 0 || (rest.data?.length ?? 0) > 0;
}
