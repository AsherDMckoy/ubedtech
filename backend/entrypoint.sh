#!/bin/sh
# Container entrypoint: bring the demo database to ready, then serve.
#
# `backend seed-demo` does everything in one shot — connects, runs the
# embedded migrations, applies seed.sql if the core rows are missing,
# layers the demo dataset (idempotent: skipped when already present).
# It fails fast while PostgreSQL is still starting, so retry in a
# bounded loop; compose's depends_on healthcheck makes this a rare path.
set -eu

tries=0
until /app/backend seed-demo; do
    tries=$((tries + 1))
    if [ "$tries" -ge 30 ]; then
        echo "database never became ready after $tries attempts" >&2
        exit 1
    fi
    echo "database not ready yet (attempt $tries), retrying..." >&2
    sleep 2
done

exec /app/backend
