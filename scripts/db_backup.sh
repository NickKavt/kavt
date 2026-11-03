#!/usr/bin/env bash
set -euo pipefail
: "${DATABASE_URL:?DATABASE_URL is not set}"
DIR="${BACKUP_DIR:-/var/backups/budget}"; KEEP="${KEEP_COUNT:-14}"
F="$DIR/budget_$(date +%Y%m%d_%H%M%S).dump"
pg_dump --dbname="$DATABASE_URL" --format=custom --no-owner --no-acl --file="$F"
pg_restore --list "$F" >/dev/null
ls -1t "$DIR"/budget_*.dump | tail -n +$((KEEP+1)) | xargs -r rm -f
echo "backup OK: $F"
