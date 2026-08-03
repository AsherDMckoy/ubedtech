#!/bin/sh
# One command to a running, seeded demo:
#   ./demo.sh          build + launch + wait + smoke-check, prints credentials
#   ./demo.sh fresh    same, but wipes the database first (known-good stage)
#   ./demo.sh down     stop everything (data kept; use fresh to reset)
#
# Works with podman (preferred) or docker; compose comes from
# `<runtime> compose` or a standalone podman-compose/docker-compose.
set -eu

if command -v podman >/dev/null 2>&1; then
    runtime=podman
elif command -v docker >/dev/null 2>&1; then
    runtime=docker
else
    echo "error: neither podman nor docker found. Install podman (and podman-compose)." >&2
    exit 1
fi

if "$runtime" compose version >/dev/null 2>&1; then
    compose="$runtime compose"
elif command -v "$runtime-compose" >/dev/null 2>&1; then
    compose="$runtime-compose"
else
    echo "error: no compose provider. Install podman-compose (or docker-compose)." >&2
    exit 1
fi

case "${1:-up}" in
    down)
        $compose down
        exit 0
        ;;
    fresh)
        $compose down -v || true
        ;;
    up) ;;
    *)
        echo "usage: ./demo.sh [up|fresh|down]" >&2
        exit 1
        ;;
esac

$compose up --build -d

printf "waiting for the app to come up"
i=0
while [ "$i" -lt 90 ]; do
    if curl -fsS -o /dev/null http://127.0.0.1:8080/ui/login 2>/dev/null; then
        # Smoke check: a real sign-in must round-trip (303 = success PRG).
        code=$(curl -s -o /dev/null -w '%{http_code}' -c /dev/null \
            -d 'username=demo.student&password=ub-demo-password' \
            http://127.0.0.1:8080/ui/login)
        if [ "$code" = "303" ]; then
            echo
            echo "READY  →  http://127.0.0.1:8080/ui/login"
            echo
            echo "  student:    demo.student    / ub-demo-password"
            echo "  instructor: demo.instructor / ub-demo-password"
            echo "  registrar:  demo.registrar  / ub-demo-password"
            echo
            echo "Guide: backend/docs/DEMO_WALKTHROUGH.md"
            echo "Reset to a known-good stage: ./demo.sh fresh"
            exit 0
        fi
        echo
        echo "app answered but sign-in returned $code (expected 303); logs:" >&2
        $compose logs --tail 40 app >&2
        exit 1
    fi
    printf .
    i=$((i + 1))
    sleep 2
done
echo
echo "app did not come up in time; logs:" >&2
$compose logs --tail 60 app db >&2
exit 1
