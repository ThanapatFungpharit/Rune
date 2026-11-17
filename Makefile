# Build and install rune-tui like any other program.
#
#   make                         # release build (target/release/rune-tui)
#   make install                 # into ~/.local (no root needed)
#   sudo make install PREFIX=/usr/local
#   make uninstall               # same PREFIX as the install
#   make dist                    # dist/rune-tui-<target>.tar.gz (+ .sha256) for install.sh
#
# Packagers: `make build` then `make install PREFIX=/usr DESTDIR=$pkgdir`.
# Only program files are installed: your session, messages and plugins live
# in ~/.local/share/rune and are never touched by install or uninstall.

PREFIX  ?= $(HOME)/.local
DESTDIR ?=
CARGO   ?= cargo
# A Rust target triple to cross-build for (e.g. x86_64-apple-darwin); empty = this machine.
TARGET  ?=

BINDIR  := $(DESTDIR)$(PREFIX)/bin
DATADIR := $(DESTDIR)$(PREFIX)/share
MANDIR  := $(DATADIR)/man/man1
APPDIR  := $(DATADIR)/applications
ICONDIR := $(DATADIR)/icons/hicolor/scalable/apps

ifeq ($(TARGET),)
BIN := target/release/rune-tui
else
BIN := target/$(TARGET)/release/rune-tui
endif

.PHONY: all build install uninstall dist clean

all: build

build:
	$(CARGO) build --release --locked --features tui --bin rune-tui $(if $(TARGET),--target $(TARGET))

# Builds first (cargo skips it when nothing changed), except as root with a
# binary already built: `sudo make install` after `make` never runs cargo as root.
install:
	@if [ "$$(id -u)" != 0 ] || [ ! -x $(BIN) ]; then $(MAKE) --no-print-directory build; fi
	install -d $(BINDIR) $(MANDIR) $(APPDIR) $(ICONDIR) $(DATADIR)/licenses/rune
	install -m 755 $(BIN) $(BINDIR)/rune-tui
	install -m 644 packaging/rune-tui.1 $(MANDIR)/rune-tui.1
	install -m 644 packaging/rune-tui.desktop $(APPDIR)/rune-tui.desktop
	install -m 644 packaging/rune-tui.svg $(ICONDIR)/rune-tui.svg
	install -m 644 LICENSE $(DATADIR)/licenses/rune/LICENSE
	@if [ -z "$(DESTDIR)" ]; then \
		command -v update-desktop-database >/dev/null && update-desktop-database -q $(APPDIR) || true; \
		command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -qtf $(DATADIR)/icons/hicolor || true; \
	fi
	@case ":$$PATH:" in *":$(PREFIX)/bin:"*) ;; \
		*) echo "note: $(PREFIX)/bin is not on your PATH; add it to run rune-tui by name" ;; esac

uninstall:
	rm -f $(BINDIR)/rune-tui $(MANDIR)/rune-tui.1 $(APPDIR)/rune-tui.desktop \
		$(ICONDIR)/rune-tui.svg $(DATADIR)/licenses/rune/LICENSE
	-rmdir $(DATADIR)/licenses/rune 2>/dev/null

# The archive install.sh deploys (a GitHub release holds one per target).
dist:
	scripts/dist.sh dist

clean:
	$(CARGO) clean
