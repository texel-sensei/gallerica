lint:
    cargo fmt --check
    cargo clippy --all-targets --all-features -- -A clippy::type_complexity -W clippy::doc_markdown -D warnings
