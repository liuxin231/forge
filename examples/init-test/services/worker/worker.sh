#!/bin/sh
echo "[worker] starting background worker..."
touch /tmp/forge-worker-alive
trap 'rm -f /tmp/forge-worker-alive; echo "[worker] stopped"; exit 0' INT TERM

i=0
while true; do
    i=$((i + 1))
    echo "[worker] processing job #$i"
    sleep 5
done
