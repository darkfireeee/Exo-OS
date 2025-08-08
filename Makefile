# Makefile principal pour Exo-OS

.PHONY: all clean build run test

all: build

build:
	cargo build --target x86_64-exo_os

run: build
	cargo run --target x86_64-exo_os

test:
	cargo test --target x86_64-exo_os

clean:
	cargo clean
