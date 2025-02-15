#/bin/bash
RUSTFLAGS='--cfg core="0"' cargo clippy --examples
RUSTFLAGS='--cfg core="1"' cargo clippy --examples