FROM rust:1-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src/ src/
COPY assets/ assets/
COPY skills/ skills/

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates git && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/oobo /usr/local/bin/oobo

ENTRYPOINT ["oobo"]
