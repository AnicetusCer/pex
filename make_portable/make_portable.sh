#!/usr/bin/env bash
# Linux packaging helper for Pex portable bundle.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

case "$(uname -s)" in
    Linux*) ;;
    *)
        echo "This packaging script supports Linux only. Use release/package.ps1 on Windows." >&2
        exit 1
        ;;
esac

usage() {
    cat <<'EOF'
Usage: ./package.sh [options]

Options:
  -b, --binary-path PATH   Path to the compiled pex binary (defaults to target/release/pex)
  -o, --output-dir NAME    Output directory name relative to the script (default: dist)
  -z, --zip                Produce pex-portable.zip alongside the dist folder
  -n, --zip-name NAME      Custom name for the zip asset (default: pex-portable.zip)
  -h, --help               Show this help message and exit
EOF
}

BINARY_PATH=""
OUTPUT_DIR="dist"
ZIP_REQUESTED=0
ZIP_NAME="pex-portable.zip"

while [[ $# -gt 0 ]]; do
    case "$1" in
        -b|--binary-path)
            [[ $# -ge 2 ]] || { echo "Option $1 requires a path argument." >&2; exit 1; }
            BINARY_PATH="$2"
            shift 2
            ;;
        -o|--output-dir)
            [[ $# -ge 2 ]] || { echo "Option $1 requires a name argument." >&2; exit 1; }
            OUTPUT_DIR="$2"
            shift 2
            ;;
        -z|--zip)
            ZIP_REQUESTED=1
            shift
            ;;
        -n|--zip-name)
            [[ $# -ge 2 ]] || { echo "Option $1 requires a name argument." >&2; exit 1; }
            ZIP_NAME="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [[ -z "$BINARY_PATH" ]]; then
    release_dir="$REPO_ROOT/target/release"
    for candidate in "pex" "pex.exe"; do
        if [[ -e "$release_dir/$candidate" ]]; then
            BINARY_PATH="$release_dir/$candidate"
            break
        fi
    done
    [[ -n "$BINARY_PATH" ]] || BINARY_PATH="$release_dir/pex"
fi

if [[ "$BINARY_PATH" != /* ]]; then
    if [[ -e "$SCRIPT_DIR/$BINARY_PATH" ]]; then
        BINARY_PATH="$(realpath "$SCRIPT_DIR/$BINARY_PATH")"
    elif [[ -e "$REPO_ROOT/$BINARY_PATH" ]]; then
        BINARY_PATH="$(realpath "$REPO_ROOT/$BINARY_PATH")"
    else
        BINARY_PATH="$(realpath "$BINARY_PATH" 2>/dev/null || true)"
    fi
fi

if [[ -z "$BINARY_PATH" ]] || [[ ! -f "$BINARY_PATH" ]]; then
    echo "Binary not found. Build with 'cargo build --release' or provide --binary-path." >&2
    exit 1
fi

if [[ "$OUTPUT_DIR" = /* ]]; then
    DIST_PATH="$OUTPUT_DIR"
else
    DIST_PATH="$SCRIPT_DIR/$OUTPUT_DIR"
fi

if [[ -d "$DIST_PATH" ]]; then
    rm -rf "$DIST_PATH"
fi
mkdir -p "$DIST_PATH"

CONFIG_SRC="$SCRIPT_DIR/config.json"
README_SRC="$SCRIPT_DIR/README.md"
[[ -f "$CONFIG_SRC" ]] || { echo "Missing config.json in $SCRIPT_DIR." >&2; exit 1; }
[[ -f "$README_SRC" ]] || { echo "Missing README.txt in $SCRIPT_DIR." >&2; exit 1; }

cp "$CONFIG_SRC" "$DIST_PATH/config.json"
cp "$README_SRC" "$DIST_PATH/README.md"
[ -f "$REPO_ROOT/LICENSE" ] && cp "$REPO_ROOT/LICENSE" "$DIST_PATH/LICENSE"
[ -f "$REPO_ROOT/NOTICE" ] && cp "$REPO_ROOT/NOTICE" "$DIST_PATH/NOTICE"
cp "$BINARY_PATH" "$DIST_PATH/$(basename "$BINARY_PATH")"

if (( ZIP_REQUESTED )); then
    command -v zip >/dev/null 2>&1 || { echo "'zip' utility not found; install it or omit --zip." >&2; exit 1; }

    [[ "$ZIP_NAME" == *.zip ]] || ZIP_NAME="${ZIP_NAME}.zip"

    if [[ "$ZIP_NAME" = /* ]]; then
        ZIP_OUTPUT="$ZIP_NAME"
        FINAL_ZIP_PATH="$ZIP_OUTPUT"
    else
        ZIP_OUTPUT="$SCRIPT_DIR/$ZIP_NAME"
        FINAL_ZIP_PATH="$DIST_PATH/$ZIP_NAME"
    fi

    rm -f "$ZIP_OUTPUT"
    (
        cd "$DIST_PATH"
        zip -r "$ZIP_OUTPUT" . >/dev/null
    )

    if [[ "$ZIP_OUTPUT" != "$FINAL_ZIP_PATH" ]]; then
        rm -f "$FINAL_ZIP_PATH"
        mv "$ZIP_OUTPUT" "$FINAL_ZIP_PATH"
    fi

    echo "Created $FINAL_ZIP_PATH"
else
    echo "Portable bundle staged in $DIST_PATH"
fi
