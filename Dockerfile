FROM rust:latest as builder

COPY ./ /work

WORKDIR /work

RUN cargo build --release

FROM gcr.io/distroless/cc

WORKDIR /

COPY --from=builder /work/target/release/slice /slice

USER nonroot

ENTRYPOINT ["/slice"]
