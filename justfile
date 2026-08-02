alias is := install-luna

default:
    @just --list

install-luna:
    cargo install --path crates/luna --bin luna --force --root "$HOME/.local"

test-luna-installer:
    scripts/test-install-luna.sh

bump-version VERSION MANIFEST:
    python3 scripts/bump-package-version.py {{ MANIFEST }} {{ VERSION }}

prepare-sdk-release VERSION:
    python3 scripts/update-sdk-release-metadata.py {{ VERSION }}

publish-crate CRATE:
    cargo publish -p {{ CRATE }}
