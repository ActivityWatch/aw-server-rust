.PHONY: all aw-server aw-webui build install package

all: aw-server aw-webui
build: aw-server

DESTDIR :=
PREFIX := /usr/local

aw-server:
	cargo build --release --bin aw-server

aw-webui:
	make -C ./aw-webui build

package:
	# Clean and prepare target/package folder
	rm -rf target/package
	mkdir -p target/package
	# Copy binary
	cp target/release/aw-server target/package
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
