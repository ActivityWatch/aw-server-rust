.PHONY: all aw-server aw-webui build install package test install-pre-commit pre-commit

all: build
build: aw-server aw-webui

DESTDIR :=
PREFIX := /usr/local

aw-server:
	cargo build --release --bin aw-server

aw-webui:
	make -C ./aw-webui build

install-git-hooks:
	@printf "make pre-commit\n" > .git/hooks/pre-commit
	@printf "make pre-push\n" > .git/hooks/pre-push
	@chmod +x .git/hooks/pre-commit .git/hooks/pre-push
	@printf "Hooks installed\n"

pre-commit:
	@cargo fmt -- --check || (printf "Error: Run cargo fmt before committing\n"; exit 1)

pre-push: pre-commit
	@cargo clippy || (printf "Error: Clippy reported error(s)\n"; exit 1)
	@cargo test || (printf "Error: Some test(s) failed\n"; exit 1)

test:
	cargo test

package:
	# Clean and prepare target/package folder
	rm -rf target/package
	mkdir -p target/package
	# Copy binary
	cp target/release/aw-server target/package/aw-server-rust
	# Copy webui assets
	cp -rf aw-webui/dist target/package/static
	# Copy service file
	cp -f aw-server.service target/package/aw-server.service

install:
	# Install aw-server executable
	mkdir -p $(DESTDIR)$(PREFIX)/bin/
	install -m 755 target/release/aw-server $(DESTDIR)$(PREFIX)/bin/aw-server
	# Install static web-ui files
	mkdir -p $(DESTDIR)$(PREFIX)/share/aw-server/static
	cp -rf aw-webui/dist/* $(DESTDIR)$(PREFIX)/share/aw-server/static
	# Install systemd user service
	mkdir -p $(DESTDIR)$(PREFIX)/lib/systemd/user
	install -m 644 aw-server.service $(DESTDIR)$(PREFIX)/lib/systemd/user/aw-server.service
