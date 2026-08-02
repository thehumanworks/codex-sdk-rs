#!/bin/sh
set -eu

repository_root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/luna-installer-test.XXXXXX")
cleanup() {
    rm -rf "$fixture"
}
trap cleanup EXIT HUP INT TERM

case "$(uname -s):$(uname -m)" in
    Darwin:arm64|Darwin:aarch64) target="aarch64-apple-darwin" ;;
    Darwin:x86_64) target="x86_64-apple-darwin" ;;
    Linux:x86_64|Linux:amd64) target="x86_64-unknown-linux-gnu" ;;
    *) printf '%s\n' "installer fixture does not support this host" >&2; exit 3 ;;
esac

tag="luna-v0.0.0-test"
archive="$tag-$target.tar.gz"
printf '%s\n' '#!/bin/sh' 'printf "%s\n" "fixture luna"' > "$fixture/luna"
chmod 755 "$fixture/luna"
tar -czf "$fixture/$archive" -C "$fixture" luna
case "$(uname -s)" in
    Darwin) digest=$(shasum -a 256 "$fixture/$archive" | awk '{print $1}') ;;
    *) digest=$(sha256sum "$fixture/$archive" | awk '{print $1}') ;;
esac
printf '%s  %s\n' "$digest" "$archive" > "$fixture/SHA256SUMS"

install_output=$("$repository_root/scripts/install-luna.sh" \
    --version "$tag" \
    --to "$fixture/bin" \
    --base-url "file://$fixture")
printf '%s\n' "$install_output" | grep -q "Installed Luna $tag"
printf '%s\n' "$install_output" | grep -q "Add $fixture/bin to PATH"

output=$("$fixture/bin/luna")
[ "$output" = "fixture luna" ] || { printf '%s\n' "installed fixture did not run" >&2; exit 1; }

# Existing installs are replaced atomically by another verified copy.
"$repository_root/scripts/install-luna.sh" \
    --version "$tag" \
    --to "$fixture/bin" \
    --base-url "file://$fixture" >/dev/null
output=$("$fixture/bin/luna")
[ "$output" = "fixture luna" ] || { printf '%s\n' "replacement fixture did not run" >&2; exit 1; }

# A modified archive must never be installed.
printf '%s' "corrupt" >> "$fixture/$archive"
set +e
"$repository_root/scripts/install-luna.sh" \
    --version "$tag" \
    --to "$fixture/corrupt-bin" \
    --base-url "file://$fixture" >/dev/null 2>&1
status=$?
set -e
[ "$status" -eq 4 ] || { printf '%s\n' "checksum mismatch should exit 4, got $status" >&2; exit 1; }
[ ! -e "$fixture/corrupt-bin/luna" ] || { printf '%s\n' "corrupt archive was installed" >&2; exit 1; }

printf '%s\n' "installer fixture passed"
