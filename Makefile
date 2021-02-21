.PHONY: all aw-server aw-webui build install package test test-coverage coverage coverage-html coverage-lcov

all: build
build: aw-server aw-webui

DESTDIR :=
PREFIX := /usr/local

# Build in release mode by default, unless RELEASE=false
ifeq ($(RELEASE), false)
	cargoflag :=
	targetdir := debug
else
	cargoflag := --release
	targetdir := release
endif

aw-server:
	cargo build $(cargoflag) --bin aw-server

aw-webui:
ifndef SKIP_WEBUI  # Skip building webui if SKIP_WEBUI is defined
	make -C ./aw-webui build
endif

test:
	cargo test

test-coverage:
ifndef COVERAGE_CACHE
	# We need to remove build files in case a non-coverage test has been run
	# before without RUST/CARGO flags needed for coverage
	rm -rf target/debug
endif
	rm -rf **/*.profraw
	# Build and test
	env RUSTFLAGS="-Zinstrument-coverage" \
	    LLVM_PROFILE_FILE="grcov-%p-%m.profraw" \
	    cargo test --verbose

GRCOV_PARAMS=$(shell find . -name "grcov-*.profraw" -print) --binary-path=./target/debug/aw-server -s . --llvm --branch --ignore-not-existing

coverage-html: test-coverage
	grcov ${GRCOV_PARAMS} -t html -o ./target/debug/$@/
	rm -rf **/*.profraw

coverage-lcov: test-coverage
	grcov ${GRCOV_PARAMS} -t lcov -o ./target/debug/lcov.info
	rm -rf **/*.profraw

coverage: coverage-html

package:
	# Clean and prepare target/package folder
	rm -rf target/package
	mkdir -p target/package
	# Copy binary
	cp target/$(targetdir)/aw-server target/package/aw-server-rust
	# Copy webui assets
	cp -rf aw-webui/dist target/package/static
	# Copy service file
	cp -f aw-server.service target/package/aw-server.service

install:
	# Install aw-server executable
	mkdir -p $(DESTDIR)$(PREFIX)/bin/
	install -m 755 target/$(targetdir)/aw-server $(DESTDIR)$(PREFIX)/bin/aw-server
	# Install static web-ui files
	mkdir -p $(DESTDIR)$(PREFIX)/share/aw-server/static
	cp -rf aw-webui/dist/* $(DESTDIR)$(PREFIX)/share/aw-server/static
	# Install systemd user service
	mkdir -p $(DESTDIR)$(PREFIX)/lib/systemd/user
	install -m 644 aw-server.service $(DESTDIR)$(PREFIX)/lib/systemd/user/aw-server.service

clean:
	cargo clean
