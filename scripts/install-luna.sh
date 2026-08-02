#!/bin/sh
set -eu

repository="thehumanworks/codex-sdk-rs"
version=""
destination="${HOME}/.local/bin"
base_url=""

usage() {
    printf '%s\n' "Usage: install-luna.sh --version VERSION [--to DIRECTORY] [--base-url URL]"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --version)
            [ "$#" -ge 2 ] || { printf '%s\n' "missing value for --version" >&2; exit 2; }
            version=$2
            shift 2
            ;;
        --to)
            [ "$#" -ge 2 ] || { printf '%s\n' "missing value for --to" >&2; exit 2; }
            destination=$2
            shift 2
            ;;
        --base-url)
            [ "$#" -ge 2 ] || { printf '%s\n' "missing value for --base-url" >&2; exit 2; }
            base_url=${2%/}
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            printf '%s\n' "unknown option: $1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

[ -n "$version" ] || { printf '%s\n' "--version is required; mutable latest installs are not supported" >&2; exit 2; }
case "$version" in
    latest|LATEST) printf '%s\n' "use an immutable Luna version, not latest" >&2; exit 2 ;;
esac

case "$version" in
    luna-v*) tag=$version ;;
    v*) tag="luna-$version" ;;
    *) tag="luna-v$version" ;;
esac

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
    Darwin:arm64|Darwin:aarch64) target="aarch64-apple-darwin" ;;
    Darwin:x86_64) target="x86_64-apple-darwin" ;;
    Linux:x86_64|Linux:amd64) target="x86_64-unknown-linux-gnu" ;;
    *)
        printf '%s\n' "unsupported platform: $os $arch" >&2
        printf '%s\n' "Supported installer targets: macOS arm64/x86_64 and Linux x86_64 (glibc)." >&2
        exit 3
        ;;
esac

archive="$tag-$target.tar.gz"
if [ -z "$base_url" ]; then
    base_url="https://github.com/$repository/releases/download/$tag"
fi

temporary_directory=$(mktemp -d "${TMPDIR:-/tmp}/luna-install.XXXXXX")
staged_path=""
cleanup() {
    if [ -n "$staged_path" ]; then
        rm -f "$staged_path"
    fi
    rm -rf "$temporary_directory"
}
trap cleanup EXIT HUP INT TERM

curl --fail --silent --show-error --location --proto '=https,file' --proto-redir '=https' --tlsv1.2 \
    --output "$temporary_directory/$archive" "$base_url/$archive"
curl --fail --silent --show-error --location --proto '=https,file' --proto-redir '=https' --tlsv1.2 \
    --output "$temporary_directory/SHA256SUMS" "$base_url/SHA256SUMS"

expected=$(awk -v file="$archive" '$2 == file { print $1; exit }' "$temporary_directory/SHA256SUMS")
[ -n "$expected" ] || { printf '%s\n' "checksum entry missing for $archive" >&2; exit 4; }
case "$os" in
    Darwin) actual=$(shasum -a 256 "$temporary_directory/$archive" | awk '{print $1}') ;;
    *) actual=$(sha256sum "$temporary_directory/$archive" | awk '{print $1}') ;;
esac
[ "$actual" = "$expected" ] || { printf '%s\n' "checksum mismatch for $archive" >&2; exit 4; }

archive_entries=$(tar -tzf "$temporary_directory/$archive")
printf '%s\n' "$archive_entries" | grep -qx 'luna' \
    || { printf '%s\n' "archive does not contain luna" >&2; exit 4; }
if printf '%s\n' "$archive_entries" | grep -Evx 'luna|README.md' >/dev/null; then
    printf '%s\n' "archive contains unexpected paths" >&2
    exit 4
fi
tar -xzf "$temporary_directory/$archive" -C "$temporary_directory" luna
[ ! -L "$temporary_directory/luna" ] \
    || { printf '%s\n' "archive contains a symlinked luna executable" >&2; exit 4; }
[ -f "$temporary_directory/luna" ] \
    || { printf '%s\n' "archive does not contain a regular luna executable" >&2; exit 4; }
[ ! -L "$destination" ] \
    || { printf '%s\n' "installation destination cannot be a symlink" >&2; exit 4; }
mkdir -p "$destination"
install_path="$destination/luna"
staged_path=$(mktemp "$destination/.luna-install.XXXXXX")
cp "$temporary_directory/luna" "$staged_path"
chmod 755 "$staged_path"
mv -f "$staged_path" "$install_path"
staged_path=""

printf '%s\n' "Installed Luna $tag to $install_path"
case ":${PATH}:" in
    *":$destination:"*) ;;
    *) printf '%s\n' "Add $destination to PATH before invoking luna." ;;
esac
printf '%s\n' "Next: luna doctor --summary"
