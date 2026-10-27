#!/usr/bin/env bash
# Runs ON the Ubuntu server, called by scripts/deploy/server.ps1 and server-backup.ps1.
#   kb-server-remote.sh prepare <dir>
#   kb-server-remote.sh backup  <dir>
#   kb-server-remote.sh up      <dir> <tag> [bind] [port]
# Never prints secrets except the one-time invite token (line "KB_FIRST_TOKEN=...").
set -eu

cmd=${1:?command}
dir=${2:?dir}
VOLUME=kb-server_kb-data   # compose file has `name: kb-server`
cd "$dir"

setenv() { # setenv KEY VALUE: add or replace one line of .env, touching nothing else
  if grep -q "^$1=" .env; then
    sed -i "s|^$1=.*|$1=$2|" .env
  else
    [ -n "$(tail -c1 .env)" ] && echo >> .env
    echo "$1=$2" >> .env
  fi
}
getenv() { grep "^$1=" .env 2>/dev/null | tail -n1 | cut -d= -f2- || true; }

case "$cmd" in
prepare)
  mkdir -p backups
  if [ -f .env ]; then
    echo "ENV=existing"
  else
    umask 077
    token=$(openssl rand -hex 24)
    printf 'KB_INVITE_TOKENS=%s\n' "$token" > .env
    chmod 600 .env
    echo "ENV=created"
    echo "KB_FIRST_TOKEN=$token"
  fi
  if docker volume inspect "$VOLUME" >/dev/null 2>&1; then echo "PREVIOUS=yes"; else echo "PREVIOUS=no"; fi
  ;;
backup)
  mkdir -p backups
  if ! docker volume inspect "$VOLUME" >/dev/null 2>&1; then echo "BACKUP=none (no data volume yet)"; exit 0; fi
  ts=$(date +%Y%m%d-%H%M%S)
  docker run --rm -e "OWNER=$(id -u):$(id -g)" -e "TS=$ts" \
    -v "$VOLUME":/data -v "$PWD/backups":/backup alpine \
    sh -c 'apk add --no-cache sqlite >/dev/null && sqlite3 /data/server.db ".backup /backup/server-$TS.db" && chown "$OWNER" "/backup/server-$TS.db"'
  ls -1t backups/server-*.db 2>/dev/null | tail -n +11 | xargs -r rm -f
  echo "BACKUP=$dir/backups/server-$ts.db"
  ;;
up)
  tag=${3:?tag}; bind=${4:-}; port=${5:-}
  [ -f .env ] || { echo "no .env: run prepare first" >&2; exit 1; }
  setenv KB_TAG "$tag"
  [ -n "$bind" ] && setenv KB_BIND "$bind"
  [ -n "$port" ] && setenv KB_PORT "$port"
  docker compose up -d
  b=$(getenv KB_BIND); b=${b:-0.0.0.0}; p=$(getenv KB_PORT); p=${p:-8787}
  echo "BIND=$b"
  echo "PORT=$p"
  host=$b; [ "$host" = "0.0.0.0" ] && host=127.0.0.1
  i=0
  while [ "$i" -lt 20 ]; do
    if json=$(curl -fsS --max-time 3 "http://$host:$p/v2/server" 2>/dev/null); then
      echo "HEALTH_JSON=$json"
      docker image prune -f >/dev/null 2>&1 || true
      exit 0
    fi
    i=$((i+1)); sleep 1
  done
  echo "HEALTH=failed (http://$host:$p/v2/server)" >&2
  docker compose ps >&2 || true
  docker compose logs --tail 30 kb-server >&2 || true
  exit 1
  ;;
*) echo "unknown command $cmd" >&2; exit 2 ;;
esac
