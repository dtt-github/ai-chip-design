#!/usr/bin/env bash
# Gives the development accounts their projects back after Rauthy starts afresh (a reset of the
# wafersmith_rauthy-data volume, or the move from SiliconCommons). Rauthy gives every account a new
# subject, and the API knows users by subject, so signing in would create a new, empty user.
# This looks up each bootstrap account's new subject (password grant on the dev-only client) and
# moves the existing user, found by its email or its SiliconCommons email, onto it.
# Run it before signing in in the browser.
set -euo pipefail

cd "$(dirname "$0")/.."
ISSUER="${WAFER_OIDC_ISSUER:-http://localhost:8180/auth/v1/}"
PG="${WAFER_PG_CONTAINER:-wafersmith-postgres-1}"

python3 -I - "$ISSUER" <<'EOF' | while read -r email old_email sub; do
import base64, json, sys, urllib.parse, urllib.request
issuer = sys.argv[1].rstrip("/")
clients = json.load(open("deploy/compose/rauthy/bootstrap/clients.json"))
cli = next(c for c in clients if c["id"] == "wafersmith-dev-cli")
for u in json.load(open("deploy/compose/rauthy/bootstrap/users.json")):
    form = {
        "grant_type": "password",
        "client_id": cli["id"],
        "client_secret": cli["secret"]["Plain"],
        "username": u["email"],
        "password": u["password"]["Plain"],
    }
    res = urllib.request.urlopen(f"{issuer}/oidc/token", urllib.parse.urlencode(form).encode())
    part = json.load(res)["access_token"].split(".")[1]
    sub = json.loads(base64.urlsafe_b64decode(part + "=" * (-len(part) % 4)))["sub"]
    print(u["email"], u["email"].replace("@wafersmith.dev", "@siliconcommons.dev"), sub)
EOF
  taken=$(docker exec "$PG" psql -U wafer -d wafersmith -qAtc \
    "SELECT email FROM users WHERE oidc_subject = '$sub'
     AND lower(email) NOT IN (lower('$email'), lower('$old_email'))")
  if [ -n "$taken" ]; then
    echo "$email: already signed in as a separate user ($taken); not changed" >&2
    continue
  fi
  n=$(docker exec "$PG" psql -U wafer -d wafersmith -qAtc \
    "UPDATE users SET oidc_subject = '$sub', email = '$email'
     WHERE lower(email) IN (lower('$email'), lower('$old_email')) RETURNING id" | wc -l)
  echo "$email: $n user(s) moved to the new subject"
done
