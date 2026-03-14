alias is := install-spark

default:
    @just --list

install-spark:
    cargo install --path crates/spark --bin spark --force --root "$HOME/.local"

bump-version VERSION MANIFEST:
    python3 scripts/bump-package-version.py {{MANIFEST}} {{VERSION}}

prepare-sdk-release VERSION:
    python3 scripts/update-sdk-release-metadata.py {{VERSION}}

publish-crate CRATE:
    cargo publish -p {{CRATE}}
