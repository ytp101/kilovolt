# Stage 1: Build stage (always runs natively on the builder platform, avoiding QEMU emulation)
FROM --platform=$BUILDPLATFORM rust:1.91-slim AS builder
WORKDIR /usr/src/kilovolt

# Install compilation tools and target architectures dependencies for cross-compiling.
# We add both architectures so we can cross compile in either direction (amd64 <-> arm64)
# without package conflicts or host dependencies limitations.
RUN dpkg --add-architecture amd64 && \
    dpkg --add-architecture arm64 && \
    apt-get update && \
    apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev:amd64 \
    libssl-dev:arm64 \
    g++-aarch64-linux-gnu \
    libc6-dev-arm64-cross \
    g++-x86-64-linux-gnu \
    libc6-dev-amd64-cross \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Add Rust targets for compilation
RUN rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu

# Global target compiler and pkg-config configurations for cross compilation
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    PKG_CONFIG_PATH_aarch64_unknown_linux_gnu=/usr/lib/aarch64-linux-gnu/pkgconfig \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CXX_x86_64_unknown_linux_gnu=x86_64-linux-gnu-g++ \
    PKG_CONFIG_PATH_x86_64_unknown_linux_gnu=/usr/lib/x86_64-linux-gnu/pkgconfig \
    PKG_CONFIG_ALLOW_CROSS=1

# Copy Cargo configuration files
COPY Cargo.toml Cargo.lock ./

# Create a dummy source file to pre-compile and cache dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs

# Read target architecture passed by Docker buildx
ARG TARGETARCH

# Pre-compile dependencies
RUN if [ "$TARGETARCH" = "arm64" ]; then \
      cargo build --target aarch64-unknown-linux-gnu --release; \
    else \
      cargo build --target x86_64-unknown-linux-gnu --release; \
    fi

# Remove dummy build artifacts
RUN rm -f target/*/release/deps/kilovolt*

# Copy the actual source files
COPY src ./src

# Build the production binary
RUN if [ "$TARGETARCH" = "arm64" ]; then \
      cargo build --target aarch64-unknown-linux-gnu --release && \
      cp target/aarch64-unknown-linux-gnu/release/kilovolt /usr/local/bin/kilovolt; \
    else \
      cargo build --target x86_64-unknown-linux-gnu --release && \
      cp target/x86_64-unknown-linux-gnu/release/kilovolt /usr/local/bin/kilovolt; \
    fi

# Stage 2: Minimal Production Runtime Stage
FROM debian:bookworm-slim
WORKDIR /app

# Install runtime certificates for HTTPS
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy the compiled binary from Stage 1
COPY --from=builder /usr/local/bin/kilovolt /usr/local/bin/kilovolt

# Expose port
EXPOSE 8080

# Execute server
CMD ["kilovolt"]