# Linux dev / CI environment for the AT-SPI surface.
#
# Provides a headless desktop accessibility stack - a virtual X server, a
# private session D-Bus, and the AT-SPI registry - plus the Rust toolchain and
# enough of GTK4 to run the Python fixture app. The entrypoint brings the stack
# up and then exec's whatever command you pass, so AT-SPI clients in that
# command see a live registry.
#
# Bind-mount the repo at /work and keep CARGO_TARGET_DIR / CARGO_HOME on Docker
# volumes so a Linux build doesn't clobber a Windows/macOS `target/` and dep
# compiles are cached across runs. See docker/linux-dev/README.md.
FROM rust:1.85-bookworm

RUN apt-get update && apt-get install -y --no-install-recommends \
        at-spi2-core \
        dbus \
        dbus-x11 \
        xvfb \
        x11-utils \
        libglib2.0-bin \
        python3 \
        python3-gi \
        gir1.2-gtk-4.0 \
        gir1.2-atspi-2.0 \
        libgtk-4-1 \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Match what CI checks.
RUN rustup component add clippy rustfmt

WORKDIR /work
COPY entrypoint.sh /usr/local/bin/agent-ctrl-linux-entrypoint
# Tolerate a CRLF checkout of the script on Windows hosts, then make it runnable.
RUN sed -i 's/\r$//' /usr/local/bin/agent-ctrl-linux-entrypoint \
    && chmod +x /usr/local/bin/agent-ctrl-linux-entrypoint

ENTRYPOINT ["/usr/local/bin/agent-ctrl-linux-entrypoint"]
CMD ["bash"]
