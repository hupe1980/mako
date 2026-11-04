// Who is at the Desk. In development the services run with the `dev-admin`
// principal; behind the gateway (oauth2-proxy) the person is the one its
// session names. Where neither answers, the Desk names nobody rather than
// guessing one.

import { useQuery } from "@tanstack/react-query";

export interface Identity {
  name: string;
  /** Where Abmelden goes; absent where nothing can sign the person out. */
  logout?: string;
}

/** oauth2-proxy's `/oauth2/userinfo`: the session's email, and its name where the IdP gave one. */
interface UserInfo {
  email?: string;
  user?: string;
  preferredUsername?: string;
}

async function gatewayIdentity(): Promise<Identity | null> {
  const res = await fetch("/oauth2/userinfo", { headers: { accept: "application/json" } });
  if (!res.ok) return null;
  const info = (await res.json()) as UserInfo;
  const name = info.preferredUsername || info.user || info.email;
  return name ? { name, logout: "/oauth2/sign_out" } : null;
}

export function useIdentity(): Identity | null {
  const gateway = useQuery({
    queryKey: ["gateway", "userinfo"],
    queryFn: gatewayIdentity,
    enabled: !import.meta.env.DEV,
    staleTime: Number.POSITIVE_INFINITY,
    retry: false,
  });
  if (import.meta.env.DEV) return { name: "dev-admin" };
  return gateway.data ?? null;
}
