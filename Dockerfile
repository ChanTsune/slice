FROM rust:slim as builder

RUN rustup target add "$(uname -m)"-unknown-linux-musl

WORKDIR /work

COPY . .

RUN cargo build --release --locked --target "$(uname -m)"-unknown-linux-musl

RUN strip /work/target/"$(uname -m)"-unknown-linux-musl/release/slice -o /slice

FROM scratch as binary

COPY --from=builder /slice /

ENTRYPOINT ["/slice"]
