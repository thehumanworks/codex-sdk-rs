alias is := install-spark

default:
    @just --list

install-spark:
    cargo install --path crates/spark --bin spark --force --root "$HOME/.local"
