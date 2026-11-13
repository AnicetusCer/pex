#!/usr/bin/env bash
# Linux packaging helper for Pex portable bundle.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(dirname "$SCRIPT_DIR")"

case "$(uname -s)" in
    Linux*) ;;
    *)
        echo "This packaging script supports Linux only. Use make_portable.ps1 on Windows." >&2
        exit 1
        ;;
esac

usage() {
    cat <<'EOF'
Usage: ./make_portable.sh [options]

Options:
  -b, --binary-path PATH   Path to the compiled pex binary (defaults to target/release/pex)
  -o, --output-dir NAME    Output directory name relative to the script (default: dist)
  -z, --zip                Produce pex-portable.zip inside the output directory
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
    BINARY_PATH="$REPO_ROOT/target/release/pex"
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

# ELF class, endianness and e_machine identify the Linux x86-64 build independently of its name.
header=$(od -An -tx1 -N20 -- "$BINARY_PATH" | tr -d '[:space:]')
if [[ "$header" != 7f454c460201????????????????????????3e00 || ! -x "$BINARY_PATH" ]]; then
    echo "Expected an executable x86-64 ELF binary: $BINARY_PATH" >&2
    exit 1
fi
if [[ "$ZIP_NAME" != /* && "$ZIP_NAME" == */* ]]; then
    echo "Use a file name or an absolute path for --zip-name." >&2
    exit 1
fi
files=(pex config.json README.md pex_snapshot.py config_example.windows config_example.linux LICENSE NOTICE)
sources=("$BINARY_PATH" "$SCRIPT_DIR/config.json" "$SCRIPT_DIR/README.md" "$SCRIPT_DIR/pex_snapshot.py"
         "$SCRIPT_DIR/config_example.windows" "$SCRIPT_DIR/config_example.linux"
         "$REPO_ROOT/LICENSE" "$REPO_ROOT/NOTICE")
for source in "${sources[@]}"; do
    [[ -f "$source" ]] || { echo "Missing bundle input: $source" >&2; exit 1; }
done
if (( ZIP_REQUESTED )); then
    command -v zip >/dev/null || { echo "'zip' is required for --zip." >&2; exit 1; }
fi
mkdir -p -- "$DIST_PATH"
if [[ ! -f "$DIST_PATH/.pex-package-stage" && -n "$(find "$DIST_PATH" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
    # Accept the previous helper's dedicated staging directory on the first upgrade.
    if [[ "$(realpath "$DIST_PATH")" != "$SCRIPT_DIR/dist/linux-x86_64" || ! -f "$DIST_PATH/pex" ]]; then
        echo "Choose an empty output directory. '$DIST_PATH' contains files from outside this packager." >&2
        exit 1
    fi
fi
touch -- "$DIST_PATH/.pex-package-stage"
for i in "${!files[@]}"; do
    cp -- "${sources[$i]}" "$DIST_PATH/${files[$i]}"
done
if (( ZIP_REQUESTED )); then
    [[ "$ZIP_NAME" == *.zip ]] || ZIP_NAME="$ZIP_NAME.zip"
    if [[ "$ZIP_NAME" = /* ]]; then zip_path="$ZIP_NAME"; else zip_path="$DIST_PATH/$ZIP_NAME"; fi
    # Use a new archive each time so old entries cannot survive a rebuild.
    temp_dir=$(mktemp -d -- "$(dirname "$zip_path")/.pex-zip.XXXXXXXX")
    trap 'rm -f -- "$temp_dir/bundle.zip"; rmdir -- "$temp_dir"' EXIT
    (cd "$DIST_PATH" && zip -q "$temp_dir/bundle.zip" "${files[@]}")
    mv -fT -- "$temp_dir/bundle.zip" "$zip_path"
    echo "Created $zip_path"
else
    echo "Portable bundle staged in $DIST_PATH"
fi
