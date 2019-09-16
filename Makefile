.PHONY: all aw-server-rust aw-webui build install package

all: aw-server-rust aw-webui
build: aw-server-rust

DESTDIR :=
PREFIX := /usr/local

aw-server-rust:
	cargo build --release

aw-webui:
	make -C ./aw-webui build

package: aw-server-rust aw-webui
	# Clean and prepare target/package folder
	rm -rf target/package
	mkdir -p target/package
	# Copy binary
	cp target/release/aw-server-rust target/package
	# Copy webui assets
	mkdir -p target/package/aw_server_rust
	cp -rf aw-webui/dist target/package/aw_server_rust/static
	# Copy service file
	cp -f aw-server-rust.service target/package/aw_server_rust

install:
	# Install aw-server-rust executable
	mkdir -p $(DESTDIR)$(PREFIX)/bin/
	install -m 755 target/release/aw-server-rust $(DESTDIR)$(PREFIX)/bin/aw-server-rust
	# Install static web-ui files
	mkdir -p $(DESTDIR)$(PREFIX)/share/aw_server_rust/static
	cp -rf aw-webui/dist/* $(DESTDIR)$(PREFIX)/share/aw_server_rust/static
	# Install systemd user service
	mkdir -p $(DESTDIR)$(PREFIX)/lib/systemd/user
	install -m 644 aw-server-rust.service $(DESTDIR)$(PREFIX)/lib/systemd/user/aw-server-rust.service
