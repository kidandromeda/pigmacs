#!/bin/bash
# Lanza pigmacs abriendo una terminal nueva
cd "$(dirname "$0")"

CMD="./target/release/pigmacs fichero.org"

if [ -n "$DISPLAY" ]; then
    if command -v xfce4-terminal >/dev/null 2>&1; then
        exec xfce4-terminal --window --title="pigmacs" --command="$CMD"
    elif command -v xterm >/dev/null 2>&1; then
        exec xterm -title pigmacs -e bash -c "$CMD"
    elif command -v tilix >/dev/null 2>&1; then
        exec tilix --title="pigmacs" -e bash -c "$CMD"
    fi
fi

exec bash -c "$CMD"
