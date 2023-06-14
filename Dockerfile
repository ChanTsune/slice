FROM rust:latest as builder

COPY ./ /work

WORKDIR /work

RUN cargo build --release

RUN strip /work/target/release/slice

FROM gcr.io/distroless/cc

WORKDIR /

COPY --from=builder /work/target/release/slice /slice

USER nonroot

ENTRYPOINT ["/slice"]
