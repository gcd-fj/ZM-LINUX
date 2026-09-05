#!/usr/bin/env bash
set -euo pipefail
# Explicit paths avoid silently compiling against another Ruffle revision.
: "${RUFFLE_ASC_JAR:?Set RUFFLE_ASC_JAR to the pinned Ruffle tools/asc/asc.jar}"
: "${RUFFLE_PLAYERGLOBAL:?Set RUFFLE_PLAYERGLOBAL to its built playerglobal_import.abc}"
project_root="$(cd "$(dirname "$0")/.." && pwd)"
build_dir="$(mktemp -d)"
trap 'rm -rf "$build_dir"' EXIT
for game in Zm4 Zm5; do
    if [[ "$game" == Zm4 ]]; then base=Preload; else base=zm5; fi
    cp "$project_root/assets/bridge/stubs/$base.as" "$build_dir/"
    cp "$project_root/assets/bridge/ZmLinux${game}Bridge.as" "$build_dir/"
    java -jar "$RUFFLE_ASC_JAR" -AS3 -import "$RUFFLE_PLAYERGLOBAL" "$build_dir/$base.as"
    java -jar "$RUFFLE_ASC_JAR" -AS3 -import "$RUFFLE_PLAYERGLOBAL" -import "$build_dir/$base.abc" "$build_dir/ZmLinux${game}Bridge.as"
    test -s "$build_dir/ZmLinux${game}Bridge.abc"
    cp "$build_dir/ZmLinux${game}Bridge.abc" "$project_root/assets/bridge/"
done
