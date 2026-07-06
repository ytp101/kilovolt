# Stage 1: Build Dependencies (for layer caching)
FROM rust:latest AS builder
WORKDIR /usr/src/kilovolt

# Create a dummy source folder and write an empty main.rs to compile dependencies first
RUN mkdir src && echo "fn main() {}" > src/main.rs
COPY Cargo.toml Cargo.lock ./

# Compile dependencies in release mode (this layer will be cached unless dependencies change)
RUN cargo build --release
RUN rm -f target/release/deps/kilovolt*

# Copy the actual source files
COPY src ./src

# Compile the production binary
RUN cargo build --release

# Stage 2: Minimal Production Runtime
FROM debian:bookworm-slim
WORKDIR /app

# Install CA certificates for upstream HTTPS connection handshakes (OpenAI)
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# Pull the compiled binary from the builder stage
COPY --from=builder /usr/src/kilovolt/target/release/kilovolt /usr/local/bin/kilovolt

# Expose the default port
EXPOSE 8080

# Run the binary
CMD ["kilovolt"]