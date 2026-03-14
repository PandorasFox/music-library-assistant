#!/bin/sh
set -e

# Ensure XDG dirs exist
mkdir -p "$XDG_CONFIG_HOME/mm" "$XDG_DATA_HOME/mm"

# Start the Witch in the background
mm &
WITCH_PID=$!

# Wait for the socket to appear
SOCKET="$XDG_RUNTIME_DIR/mm.sock"
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
