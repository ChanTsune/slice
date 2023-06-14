FROM rust:latest as builder

COPY ./ /work

WORKDIR /work

RUN cargo build --release

FROM debian:buster-slim
COPY --from=builder /work/target/release/slice /usr/local/bin/slice

ENTRYPOINT ["slice"]
