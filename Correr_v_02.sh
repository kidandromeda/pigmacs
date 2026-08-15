#!/bin/bash
# Lanza pigmacs v_02 (fase 3: + panel de IA con opencode)
# Extrae el tag v_02 del git, lo compila en builds/v_02 y abre una terminal.
set -e
cd "$(dirname "$0")"

VERSION="v_02"
BUILD="builds/$VERSION"

if [ ! -x "$BUILD/target/release/pigmacs" ]; then
    echo "Compilando $VERSION ..."
    mkdir -p builds
    rm -rf "$BUILD"
    mkdir -p "$BUILD"
    git archive "$VERSION" | tar -x -C "$BUILD"
    (cd "$BUILD" && cargo build --release 2>&1 | tail -3)
fi

FILE="$BUILD/fichero.org"
[ -f "$FILE" ] || FILE="untitled.org"
CMD="$BUILD/target/release/pigmacs $FILE"

if [ -n "$DISPLAY" ]; then
    if command -v xfce4-terminal >/dev/null 2>&1; then
        exec xfce4-terminal --window --title="pigmacs $VERSION" --command="$CMD"
    elif command -v xterm >/dev/null 2>&1; then
        exec xterm -title "pigmacs $VERSION" -e bash -c "$CMD"
    elif command -v tilix >/dev/null 2>&1; then
        exec tilix --title="pigmacs $VERSION" -e bash -c "$CMD"
    fi
fi

exec bash -c "$CMD"
