# Build: a release binary of the server.
FROM rust:1-slim-trixie AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
# Dependencies first, so changing the app's code doesn't rebuild them.
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release --locked && rm -rf src
COPY src src
COPY migrations migrations
RUN touch src/main.rs && cargo build --release --locked

# Run: the binary and the interface's files, as an unprivileged user, without a shell.
FROM gcr.io/distroless/cc-debian13:nonroot
WORKDIR /app
COPY --from=build /src/target/release/budget-server /app/budget-server
COPY app/static /app/static
ENV HOST=0.0.0.0 \
    PORT=8009 \
    BUDGET_STATIC_DIR=/app/static \
    RUST_LOG=info
EXPOSE 8009
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 CMD ["/app/budget-server", "health"]
ENTRYPOINT ["/app/budget-server"]
