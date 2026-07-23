#!/bin/sh
# Container entrypoint: inject secrets from Infisical when a machine identity
# is configured, otherwise run the app with the plain environment.
#
# Expected (all four, else plain start):
#   INFISICAL_API_URL       e.g. http://169.254.1.3:8080/api (self-hosted)
#   INFISICAL_CLIENT_ID     universal-auth machine identity
#   INFISICAL_CLIENT_SECRET universal-auth machine identity
#   INFISICAL_PROJECT_ID    project holding the secrets
# Optional:
#   INFISICAL_ENV           environment slug (default: prod)
#   INFISICAL_SECRET_PATH   folder to inject from (default: /)
set -eu

if [ -n "${INFISICAL_CLIENT_ID:-}" ] && [ -n "${INFISICAL_CLIENT_SECRET:-}" ] \
   && [ -n "${INFISICAL_PROJECT_ID:-}" ]; then
  echo "entrypoint: fetching secrets from Infisical (${INFISICAL_API_URL:-cloud})" >&2
  INFISICAL_TOKEN=$(infisical login \
    --method=universal-auth \
    --client-id="$INFISICAL_CLIENT_ID" \
    --client-secret="$INFISICAL_CLIENT_SECRET" \
    --silent --plain)
  export INFISICAL_TOKEN
  # Don't leak the identity credentials into the app environment.
  unset INFISICAL_CLIENT_ID INFISICAL_CLIENT_SECRET
  exec infisical run \
    --projectId="$INFISICAL_PROJECT_ID" \
    --env="${INFISICAL_ENV:-prod}" \
    --path="${INFISICAL_SECRET_PATH:-/}" \
    -- "$@"
fi

echo "entrypoint: no Infisical identity configured — using plain environment" >&2
exec "$@"
