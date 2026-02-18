alias is := install-spark

default:
    @just --list

install-spark:
    cargo install --path . --bin spark --force --root "$HOME/.local"
