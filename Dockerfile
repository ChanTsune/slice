FROM rust:latest as builder

RUN rustup target add x86_64-unknown-linux-musl

COPY ./ /work

WORKDIR /work

RUN cargo build --release --target x86_64-unknown-linux-musl

RUN strip /work/target/x86_64-unknown-linux-musl/release/slice

FROM gcr.io/distroless/static

WORKDIR /

COPY --from=builder /work/target/x86_64-unknown-linux-musl/release/slice /slice

USER nonroot

ENTRYPOINT ["/slice"]
