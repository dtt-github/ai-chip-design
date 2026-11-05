import { UserManager, WebStorageStateStore, type User } from "oidc-client-ts";

const authority = import.meta.env.VITE_OIDC_AUTHORITY ?? "http://localhost:8180/auth/v1/";
const clientId = import.meta.env.VITE_OIDC_CLIENT_ID ?? "siliconcommons-web";

export const userManager = new UserManager({
  authority,
  client_id: clientId,
  redirect_uri: `${window.location.origin}/auth/callback`,
  post_logout_redirect_uri: `${window.location.origin}/`,
  response_type: "code",
  scope: "openid profile email",
  automaticSilentRenew: false,
  userStore: new WebStorageStateStore({ store: window.sessionStorage }),
});

export async function currentUser(): Promise<User | null> {
  const user = await userManager.getUser();
  if (!user || user.expired) return null;
  return user;
}

export function login(returnTo: string = window.location.pathname) {
  return userManager.signinRedirect({ state: { returnTo } });
}

export function logout() {
  return userManager.signoutRedirect();
}

export function displayName(user: User): string {
  const p = user.profile;
  return p.name ?? ([p.given_name, p.family_name].filter(Boolean).join(" ") || p.email || p.sub);
}
