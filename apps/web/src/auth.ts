import { UserManager, WebStorageStateStore, type User } from "oidc-client-ts";

const authority = import.meta.env.VITE_OIDC_AUTHORITY ?? "http://localhost:8180/auth/v1/";
const clientId = import.meta.env.VITE_OIDC_CLIENT_ID ?? "wafersmith-web";

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

/** Renew this long before the access token expires, so a request doesn't go out with one about to.
 * Rauthy accepts a refresh token only from 60 s before the access token expires. */
const RENEW_BEFORE_S = 45;
let renewing: Promise<User | null> | null = null;

/** The signed-in user with a usable access token, or null. A token that has expired or is about to
 * is renewed with the refresh token, without leaving the page (a sign-in redirect would lose
 * unsaved work); only when that fails does the caller have to sign in again. */
export async function currentUser(): Promise<User | null> {
  const user = await userManager.getUser();
  if (!user) return null;
  if ((user.expires_in ?? 0) > RENEW_BEFORE_S) return user;
  if (!user.refresh_token) return user.expired ? null : user;
  renewing ??= userManager
    .signinSilent()
    .catch(() => null)
    .finally(() => {
      renewing = null;
    });
  const renewed = await renewing;
  if (renewed && !renewed.expired) return renewed;
  return user.expired ? null : user;
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
