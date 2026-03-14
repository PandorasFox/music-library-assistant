#!/bin/bash
set -e

# Ensure XDG dirs exist
mkdir -p "$XDG_CONFIG_HOME/mm" "$XDG_DATA_HOME/mm"

# Seed config.kdl from MM_ROOT if no config exists yet.
# The Witch still needs first-time setup via the web UI (user creation, etc.)
# but having config.kdl avoids a crash when a stale DB exists without config.
if [ ! -f "$XDG_CONFIG_HOME/mm/config.kdl" ] && [ -n "$MM_ROOT" ]; then
    echo "Seeding config.kdl with root=$MM_ROOT"
    printf '// Music Magic configuration (seeded by Docker)\nroot "%s"\n' "$MM_ROOT" \
        > "$XDG_CONFIG_HOME/mm/config.kdl"
fi

# Clean stale socket from previous run (bind-mounted dir persists across restarts)
SOCKET="$XDG_RUNTIME_DIR/mm.sock"
rm -f "$SOCKET"

# Start the Witch in the background
mm &
WITCH_PID=$!

# Wait for the socket to appear
echo "Waiting for Witch socket at $SOCKET ..."
for i in $(seq 1 30); do
    [ -S "$SOCKET" ] && break
    sleep 0.5
done

if [ ! -S "$SOCKET" ]; then
    echo "ERROR: Witch socket did not appear after 15s"
    kill $WITCH_PID 2>/dev/null
    exit 1
fi

echo "Witch ready. Starting mm-web ..."

# Start mm-web in the foreground
mm-web \
    --socket "$SOCKET" \
    --listen "${MM_WEB_LISTEN:-0.0.0.0:3313}" \
    --static-dir "${MM_WEB_STATIC_DIR:-/srv/mm-web/static}" &
WEB_PID=$!

# Forward signals to both processes
trap 'kill $WEB_PID $WITCH_PID 2>/dev/null; wait' TERM INT

# If either exits, tear down the other
wait -n $WITCH_PID $WEB_PID 2>/dev/null
EXIT_CODE=$?
kill $WEB_PID $WITCH_PID 2>/dev/null
wait
exit $EXIT_CODE
